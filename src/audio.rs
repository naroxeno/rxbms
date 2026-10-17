//! 全局音频管理系统：kira（cpal 后端）驱动，覆盖官方教程全部六章能力。
//!
//! 架构（对照 http://tesselo.de/kira/ 六章）：
//! - **管理器**（第 1-2 章）：`kira::AudioManager<DefaultBackend>` 单实例 Resource，
//!   后台音频线程渲染，主线程零阻塞；
//! - **播放**（第 3 章）：键音与小 BGM 采样 = `StaticSoundData`（`Arc` 共享采样、
//!   克隆廉价、可多路并发，**并发受控**——同一 keysound 最多 2 个实例同时发声
//!   （LN 密集键音停旧播新，不叠加淹没 BGM）、键音轨总量上限 32）；大 BGM
//!   （≥ 2MB）= `StreamingSoundData`（流式后台解码，整曲不占内存）；参数
//!   （音量/播放速率/声像）支持 `Tween` 平滑过渡；
//! - **混音器**（第 4 章）：main → {bgm, keysound, metronome, menu} 四条 sub track 分层。
//!   BMS 无独立打击音效（打击音会干扰键音辨识），故不设 se 轨道；
//!   节拍器独立轨道（常驻合成音，不污染键音轨的 `num_sounds` 判定）；
//! - **时钟**（第 5 章）：谱面 clock（BPM × 192 tick，对应 BMS 1/192 分辨率），
//!   BGM 以 `start_time(clock.time())` 对齐——开始时刻由音频线程精确控制，
//!   消除主线程 → 音频线程的调度抖动；
//! - **自定义 Sound**（第 6 章）：[`metronome::MetronomeSound`] 节拍器，
//!   音频线程实时合成点击音，tempo/开关经命令通道控制。
//!
//! 缓存（[`AudioCache`]）与租约（[`AudioLease`]）沿用旧架构：跨游玩 LRU +
//! 引用计数，缓存对象改为 kira 的 `StaticSoundData`（克隆零拷贝）。
//! 自写 cpal 混音器与 Symphonia 解码池删除（kira 内部实现，见 Cargo.toml 注释）。

pub mod decode;
pub mod metronome;
pub mod track;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{mpsc, Mutex},
};

use bevy::prelude::*;
use kira::{
    AudioManagerSettings, Decibels, DefaultBackend, Frame, Tween,
    clock::ClockHandle,
    clock::ClockSpeed,
    sound::{
        FromFileError, PlaybackState,
        static_sound::{StaticSoundData, StaticSoundHandle},
        streaming::{StreamingSoundData, StreamingSoundHandle},
    },
};

use crate::core::settings::SettingsStore;

use self::decode::DecodePool;
use self::metronome::{MetronomeData, MetronomeHandle};
use self::track::Tracks;

/// 缓存项数量上限（防御性兜底）。
///
/// 主内存回收机制是铺面退出时 `release` 立即释放引用归零的项；数量上限只在
/// 单场游玩内缓存异常膨胀时兜底淘汰。
const LRU_CACHE_MAX: usize = 512;

/// 缓存总字节上限（解码后的 f32 采样，`Frame` = 立体声 8 字节/帧）。
///
/// 首批（开玩所需）音频解码后 **pinned** 不可淘汰；其余音频按时间窗口渐进解码，
/// 超出本上限时按 LRU 淘汰**非 pinned** 项——被淘汰的音频再次触发时由
/// `play_synced` / `play_bgm_event` 同步解码补回，内存不随谱面音频总量线性增长。
const AUDIO_CACHE_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// 音频预加载窗口（秒）：游玩中每帧把 `now + 本值` 内的音频交后台解码池。
///
/// 旧实现一次性把**整张谱面的全部音频**交池解码（内存 = 全谱面采样之和，
/// 数千个键音可达数百 MB）。改为滚动窗口后内存只与窗口内文件数相关；
/// 4 个解码线程解码数百 KB 的键音只需毫秒级，10s 前瞻足够留出余量。
const PRELOAD_LOOKAHEAD_SECS: f64 = 10.0;

/// BGM 走流式解码的文件大小下限（字节）。
///
/// 小于该值的 BGM 通道采样（常被密集事件引用）走静态缓存多路并发，
/// 避免每次事件重开流+掐断旧流导致背景音几乎无声（2026-08 修复的 bug）。
const STREAMING_MIN_BYTES: u64 = 2_000_000;

/// BGM 走流式解码的最大事件引用次数。
///
/// 同一文件被超过该次数的事件触发（循环采样/密集 BGM 序列）即使文件够大
/// 也走静态缓存——否则每次事件停旧流重开，背景音被掐断。
const STREAMING_MAX_EVENTS: usize = 8;

/// 同一 keysound 文件在键音轨上允许的最大并发实例数。
///
/// BMS 谱面的长音持续音常由 LN body 内**密集引用同一 wav** 的键音事件实现
/// （每 16/32 分音符一个）。若每个事件都新建一个实例且互不干扰，多个实例
/// 波形叠加会**盖过 BGM**（用户报告：听不见 BGM）。超过上限时停止最旧的
/// 同路径实例再播新的——发声不丢、叠加受控。
const KEYSOUND_MAX_PER_PATH: usize = 2;

/// 键音轨并发实例总上限（极端谱面兜底；kira 轨道默认容量 128）。
///
/// 多条轨道同时按住 LN + 常规键音密集出现时，无上限会撑满轨道容量并
/// 大量堆叠；此处丢弃超限的本次发声（同时同路径限制已保证主流场景不触发）。
const KEYSOUND_MAX_TOTAL: usize = 32;

/// 连续音频（"背景床"）判据：采样时长 ≥ 铺面总时长 × 该比例。
///
/// 01 通道（BGM 轨）在 BMS 里两种用法在**触发结构上不可区分**（rainbow 类是同一
/// 整曲 BGM 被 20 次/秒地铺满，键音铺面是几百个不同采样被密集触发），能分开
/// 两类作者的只有采样时长这一物理属性：单发音效（鼓/镲/打击乐）的数秒内衰减，
/// 连续音乐床/整曲是数十秒到分钟。因此判据取"时长是否足以覆盖铺面的显著部分"，
/// 不写死绝对秒数（`10.0` 那种数看着精确，其实是随手取的）。
///
/// 代入实测：MORNINGLOOM 铺面 136s → 阈值 34s，最长鼓组 1.32s 全部判 `OneShot`
/// （此前用 1.0s 绝对阈值把 42% 鼓组事件判成 Bed 而丢弃）；整曲 BGM ≈ 铺面时长 → Bed。
///
/// 判据刻意偏保守：拿不准（铺面总时长未知/为 0）一律按采样处理——**判定错误
/// 最坏只会多叠一点，不会吞音**。
const BED_MIN_CHART_RATIO: f64 = 0.25;

/// 一次性 BGM 采样同时发声数软上限（超出时停最旧，保证新事件必发声）。
///
/// 必须小于 `track::BGM_SAMPLE_SOUND_CAPACITY`（kira 硬上限），否则会再次
/// 触发 `SoundLimitReached` 丢音。
const BGM_SAMPLE_MAX_VOICES: usize = 256;

/// 软上限必须小于 kira 子轨硬上限，否则又会退回 `SoundLimitReached` 丢音。
const _: () = assert!(BGM_SAMPLE_MAX_VOICES < track::BGM_SAMPLE_SOUND_CAPACITY);

/// 当前铺面 BGM 的播放句柄（流式大 BGM 或静态缓存 BGM 统一封装，
/// 供去重判断与停止使用）。
enum BgmHandle {
    /// 流式大 BGM（`StreamingSoundData`，省内存）。
    Streaming(StreamingSoundHandle<FromFileError>),
    /// 静态缓存 BGM（`StaticSoundData`，密集事件引用的大文件走缓存）。
    Static(StaticSoundHandle),
}

impl BgmHandle {
    /// 是否仍在播放（用于 BGM 事件去重与 `is_playing` 判定）。
    fn is_playing(&self) -> bool {
        match self {
            Self::Streaming(h) => h.state() == PlaybackState::Playing,
            Self::Static(h) => h.state() == PlaybackState::Playing,
        }
    }

    /// 停止播放。
    fn stop(&mut self) {
        match self {
            Self::Streaming(h) => h.stop(Tween::default()),
            Self::Static(h) => h.stop(Tween::default()),
        }
    }
}

/// 全局音频管理器（Resource）。
#[derive(Resource)]
pub struct AudioManager {
    /// kira 音频管理器（cpal 输出，音频线程渲染）。
    kira: kira::AudioManager<DefaultBackend>,
    /// 主界面（master）音轨：选曲/标题界面的 BGM，独立于 gameplay 生命周期。
    tracks: Tracks,
    /// 谱面时钟（BPM × 192 tick）。BGM 播放以 `clock.time()` 对齐。
    clock: ClockHandle,
    /// 节拍器（第 6 章自定义 Sound，常驻 se 轨道，tempo/开关可编程控制）。
    metronome: MetronomeHandle,
    /// 当前主界面 BGM 流（`stop_menu_bgm` 时取走停止）。
    menu_handle: Option<StreamingSoundHandle<FromFileError>>,
    /// 当前铺面正在播放的 BGM（`stop_all` 时取走停止；去重/状态查询用）。
    bgm_handle: Option<BgmHandle>,
    /// 当前铺面 BGM 的路径（去重：相同 BGM 正在播放时忽略后续事件）。
    bgm_path: Option<PathBuf>,
    /// 标记为 BGM 且**大文件**（走流式播放，不缓存）的路径。
    streaming_paths: HashSet<PathBuf>,
    /// 缓存与引用计数（与设备解耦）。
    cache: AudioCache,
    /// 后台解码池（首批之外音频渐进预加载）。
    pool: DecodePool,
    /// 解码结果队列（worker → 主线程）。
    ready_rx: Mutex<mpsc::Receiver<(PathBuf, Option<StaticSoundData>)>>,
    /// 预加载时间线：音频路径**首次**触发时间（秒）→ 路径，按时间升序。
    ///
    /// 只登记"首次触发"是为了每个文件解码一次即可（后续事件复用缓存；
    /// 被 LRU 淘汰后再次触发会同步解码补回）。谱面进入时由
    /// [`AudioManager::set_preload_timeline`] 整体替换。
    preload_timeline: Vec<(f64, PathBuf)>,
    /// 时间线游标：下一个待提交解码的下标（滚动窗口推进）。
    preload_idx: usize,
    /// 全局音量（0.0–1.0，线性 → main track 的 dB 音量）。
    volume: f32,
    /// 键音轨上正在播放的声音实例（静态或流式），
    /// 供同路径并发控制与已结束实例清理（防堆积）。
    keysound_voices: Vec<KeysoundVoice>,
    /// 键音并发限制器（同路径计数 + 总量上限，纯逻辑可测）。
    keysound_limiter: KeysoundLimiter,
    /// 本场铺面总时长（秒），连续音频判据 [`is_bed_audio`] 的分母。
    ///
    /// 由 [`AudioManager::set_song_total_secs`] 在进入铺面时写入，退出清零。
    song_total_secs: f64,
    /// 当前铺面总时长未知时的兜底：`false`＝未设置，判定一律按采样（不吞音）。
    song_total_known: bool,
    /// BGM 角色判定缓存（路径 → 是否按 Bed 语义处理）。
    ///
    /// 只在首次判定时打日志、之后命中缓存；每场铺面退出时清空。
    bgm_roles: HashMap<PathBuf, bool>,
    /// 一次性 BGM 采样轨上正在发声的实例（软上限见 [`BGM_SAMPLE_MAX_VOICES`]）。
    ///
    /// 短采样不参与去重（每个事件都播），因此必须自行管理同时发声数：
    /// 超出上限时停最旧的，避免撑满 kira 硬上限后 `play` 报
    /// `SoundLimitReached` 导致后续事件无声（用户报告的"一直在丢音"）。
    bgm_samples: Vec<StaticSoundHandle>,
}


impl AudioManager {
    /// 打开默认输出设备并初始化 kira（三条轨道 + 谱面时钟 + 节拍器）。
    ///
    /// # Errors
    ///
    /// 无可用音频设备、资源超限或后端初始化失败时返回错误。
    pub fn new() -> Result<Self, String> {
        let mut kira = kira::AudioManager::<DefaultBackend>::new(AudioManagerSettings::default())
            .map_err(|e| format!("初始化音频后端失败: {e}"))?;
        let mut tracks = Tracks::new(&mut kira)?;
        // 谱面时钟：初始 120 BPM × 192 分辨率，`begin_song` 时按谱面实际 BPM 调整
        let clock = kira
            .add_clock(ClockSpeed::TicksPerMinute(120.0 * 192.0))
            .map_err(|_| "创建谱面时钟失败".to_string())?;
        // 节拍器常驻独立轨道（默认静音，`set_metronome` 开启）
        let metronome = tracks
            .metronome
            .play(MetronomeData::default())
            .map_err(|_| "启动节拍器失败".to_string())?;
        // 有界结果通道：worker 解码完成后阻塞回传（背压防内存堆积）
        let (ready_tx, ready_rx) = mpsc::sync_channel(decode::RESULT_CHANNEL_CAPACITY);
        let pool = DecodePool::new(ready_tx);
        Ok(Self {
            kira,
            tracks,
            clock,
            metronome,
            menu_handle: None,
            bgm_handle: None,
            bgm_path: None,
            streaming_paths: HashSet::new(),
            cache: AudioCache::default(),
            pool,
            ready_rx: Mutex::new(ready_rx),
            preload_timeline: Vec::new(),
            preload_idx: 0,
            volume: 1.0,
            keysound_voices: Vec::new(),
            keysound_limiter: KeysoundLimiter::default(),
            song_total_secs: 0.0,
            song_total_known: false,
            bgm_roles: HashMap::new(),
            bgm_samples: Vec::new(),
        })
    }

    /// 谱面进入：注册 BGM 路径（大文件且稀疏引用 → 流式播放，不缓存）。
    ///
    /// **双条件分流**（文件大小 + 事件引用次数，见 [`should_stream_file`]）：
    /// BMS 的 BGM 通道事件分两类——
    /// - 真正的长 BGM：文件大（≥ [`STREAMING_MIN_BYTES`]）且事件稀疏（≤
    ///   [`STREAMING_MAX_EVENTS`]），整曲背景音乐、播放长，走 `StreamingSoundData`
    ///   流式解码（省内存）；
    /// - 其余（小采样、或大文件被密集事件引用，如 rainbow 每秒 ~20 个 BGM 事件）：
    ///   走静态缓存多路并发。否则每次事件 `play_bgm` 停旧流+重开文件+等流式缓冲，
    ///   背景音会被密集掐断而几乎无声（2026-08 修复的 bug）。
    ///
    /// 注意：必须在 `submit_priority` **之前**调用，优先级加载会跳过流式 BGM 文件。
    pub fn register_bgm(&mut self, stats: std::collections::HashMap<PathBuf, usize>) {
        self.streaming_paths.clear();
        for (path, events) in stats {
            if should_stream_file_with_events(&path, events) {
                self.streaming_paths.insert(path);
            }
        }
    }

    /// 谱面进入：获取音频租约（引用计数 +1，记录全部待加载路径）。
    ///
    /// 真正决定"何时解码"的是 [`AudioManager::set_preload_timeline`] +
    /// [`AudioManager::pump_preload`]（按谱面时间滚动窗口），本方法只登记租约
    /// （退出时按引用归零立即释放解码数据）。
    pub fn acquire(&mut self, paths: &[PathBuf]) -> AudioLease {
        self.cache.acquire(paths)
    }

    /// 提交首批高优先级加载（前 30 秒音频），**同步解码**进缓存（量小，毫秒级）。
    ///
    /// 首批项会被标记为 pinned（LRU 淘汰时不可淘汰，见 [`AudioCache`]）。
    /// BGM 文件跳过（流式播放，不缓存）。
    pub fn submit_priority(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            if !self.should_cache(&path) {
                continue; // BGM：流式播放，不缓存
            }
            self.cache.mark_priority(&path);
            if self.cache.get(&path).is_none() {
                match StaticSoundData::from_file(&path) {
                    Ok(data) => self.cache.insert_ready(path, data),
                    Err(e) => {
                        warn!("[audio] 解码失败 {}: {e}", path.display());
                        self.cache.remove_requested(&path);
                    }
                }
            }
        }
    }

    /// 登记本场铺面总时长（秒）：连续音频判据 [`is_bed_audio`] 的分母。
    ///
    /// 与 [`AudioManager::set_preload_timeline`] 一起在进入铺面时调用；
    /// 传 0 视作未知（判定退回"一律按采样，宁可叠音不可吞音"）。
    pub fn set_song_total_secs(&mut self, total_secs: f64) {
        self.song_total_secs = total_secs;
        self.song_total_known = total_secs > 0.0;
        self.bgm_roles.clear();
    }

    /// 设置预加载时间线（谱面进入时调用）：音频路径首次触发时间（秒）升序。
    ///
    /// 整体替换上一张谱面的时间线并重置游标——旧谱面残留路径不会再被提交解码。
    pub fn set_preload_timeline(&mut self, timeline: Vec<(f64, PathBuf)>) {
        self.preload_timeline = timeline;
        self.preload_idx = 0;
    }

    /// 推进预加载窗口（每帧调用）：把 `now_sec + PRELOAD_LOOKAHEAD_SECS` 之内
    /// 尚未缓存的音频交后台解码池。
    ///
    /// **内存约束**：旧实现一次性交池解码整张谱面的全部音频（数千键音 = 数百
    /// MB）；滚动窗口下同时在途/缓存的文件数只与窗口内事件数相关，配合
    /// [`AUDIO_CACHE_MAX_BYTES`] 淘汰，内存占用与谱面音频总量解耦。
    pub fn pump_preload(&mut self, now_sec: f64) {
        let due = due_preload_paths(
            &self.preload_timeline,
            &mut self.preload_idx,
            now_sec,
            PRELOAD_LOOKAHEAD_SECS,
        );
        let mut submitted = 0usize;
        for path in due {
            if !self.should_cache(&path) || self.cache.get(&path).is_some() {
                continue; // 流式 BGM 不走缓存；已缓存（首批/此前窗口）无需重复解码
            }
            self.pool.submit(path);
            submitted += 1;
        }
        if submitted > 0 {
            debug!("[audio] 预加载窗口: +{submitted}（{PRELOAD_LOOKAHEAD_SECS} 秒前瞻）");
        }
    }

    /// 收拢后台解码结果（每帧调用，非阻塞）。
    pub fn drain_ready(&mut self) {
        let rx = match self.ready_rx.lock() {
            Ok(rx) => rx,
            Err(_) => return,
        };
        let mut items = Vec::new();
        while let Ok((path, data)) = rx.try_recv() {
            items.push((path, data));
        }
        drop(rx);
        for (path, data) in items {
            match data {
                Some(data) => self.cache.insert_ready(path, data),
                // 解码失败：从待加载集合移除，避免永久等待
                None => self.cache.remove_requested(&path),
            }
        }
    }

    /// 谱面开始游玩：确保本场谱面的播放轨道存在，重置并启动谱面时钟
    /// （BPM × 192 tick）。
    ///
    /// 时钟启动后，BGM 以 `clock.time()` 对齐开始；谱面判定仍用实时时钟
    /// （`TimeStamp`），两者起点一致（loading 完成时刻）。
    pub fn begin_song(&mut self, bpm: f64) {
        self.ensure_song_tracks();
        self.clock.stop();
        self.clock
            .set_speed(ClockSpeed::TicksPerMinute(bpm * 192.0), Tween::default());
        self.clock.start();
    }

    /// 确保本场谱面的播放轨道已创建（上一场 `stop_all` 销毁后重建）。
    fn ensure_song_tracks(&mut self) {
        self.tracks.ensure_song_tracks(&mut self.kira);
    }

    /// 谱面退出：释放租约（引用计数 -1），**引用归零的立即释放解码数据**
    /// （铺面退出即回收内存，不等 LRU 上限）；清空 BGM 标记与后台待加载队列。
    pub fn release(&mut self, lease: &AudioLease) {
        self.cache.release(lease);
        self.streaming_paths.clear();
        // 清空预加载时间线：旧谱面路径不再提交解码（防止 loading 中退出
        // 的残留路径在下一场被重复解码）。
        self.preload_timeline.clear();
        self.preload_idx = 0;
        // 铺面作用域的 BGM 角色缓存/时长同样作废（下一场重新判定）
        self.bgm_roles.clear();
        self.song_total_secs = 0.0;
        self.song_total_known = false;
    }

    /// 播放一个音频（键音路径）。
    ///
    /// 多路并发**受控**：同一路径最多 [`KEYSOUND_MAX_PER_PATH`] 个实例同时发声
    /// （超出时停止最旧的再播新——LN 密集键音不叠加淹没 BGM，按键/事件也保证
    /// 发声）；全部键音合计最多 [`KEYSOUND_MAX_TOTAL`] 个。已结束的实例每帧
    /// 清理（见 [`AudioManager::prune_keysound_voices`]）。
    ///
    /// 播放形态按文件分流：普通键音/小采样 = 静态缓存（零拷贝多路并发）；
    /// 被标记为流式 BGM 的大文件（`register_bgm`）若也被键音事件引用 = 流式
    /// 播放到**键音轨**（不进静态缓存省内存，也不触碰 BGM 轨）——两种形态
    /// **统一受并发上限约束**，流式实例不因无缓存而无限叠加。
    ///
    /// 返回是否实际发起播放（解码失败、轨道缺失或总量超限时为 `false`）。
    pub fn play_synced(&mut self, path: &Path) -> bool {
        self.ensure_song_tracks();
        // 并发控制：先清理已结束实例，再为同路径腾出配额（停最旧），
        // 最后检查总量上限。
        self.prune_keysound_voices();
        self.keysound_make_room(path);
        if !self.keysound_limiter.try_acquire(path, KEYSOUND_MAX_TOTAL) {
            return false; // 总量超限（极端谱面兜底）：丢弃本次发声
        }
        let streaming = !self.should_cache(path);
        let Some(keysound) = self.tracks.keysound.as_mut() else {
            self.keysound_limiter.release(path);
            return false;
        };
        if !streaming {
            // 静态缓存：priority 预解码命中直接播；未命中同步解码一次并缓存
            let data = match self.cache.get(path) {
                Some(data) => data.clone(),
                None => match StaticSoundData::from_file(path) {
                    Ok(data) => {
                        self.cache.insert_ready(path.to_path_buf(), data.clone());
                        data
                    }
                    Err(e) => {
                        warn!("[audio] 同步解码失败 {}: {e}", path.display());
                        self.keysound_limiter.release(path);
                        return false;
                    }
                },
            };
            match keysound.play(data) {
                Ok(handle) => {
                    self.keysound_voices.push(KeysoundVoice::Static {
                        path: path.to_path_buf(),
                        handle,
                    });
                    true
                }
                Err(e) => {
                    warn!("[audio] 键音播放失败 {}: {e}", path.display());
                    self.keysound_limiter.release(path);
                    false
                }
            }
        } else {
            // 流式大文件键音：流式播放（省内存），受同一并发上限约束
            match StreamingSoundData::from_file(path) {
                Ok(data) => match keysound.play(data) {
                    Ok(handle) => {
                        self.keysound_voices.push(KeysoundVoice::Streaming {
                            path: path.to_path_buf(),
                            handle,
                        });
                        true
                    }
                    Err(e) => {
                        warn!("[audio] 键音流式播放失败 {}: {e}", path.display());
                        self.keysound_limiter.release(path);
                        false
                    }
                },
                Err(e) => {
                    warn!("[audio] 键音流式解码失败 {}: {e}", path.display());
                    self.keysound_limiter.release(path);
                    false
                }
            }
        }
    }

    /// 清理键音轨上**已结束**的声音实例（同步归还并发计数），防实例/计数堆积。
    ///
    /// 每帧调用（实例数 ≤ [`KEYSOUND_MAX_TOTAL`]，`state()` 为原子读，开销可忽略）。
    fn prune_keysound_voices(&mut self) {
        let mut i = 0;
        while i < self.keysound_voices.len() {
            if !self.keysound_voices[i].is_playing() {
                let voice = self.keysound_voices.remove(i);
                self.keysound_limiter.release(voice.path());
            } else {
                i += 1;
            }
        }
    }

    /// 同路径并发达到上限时，**停止最旧的同路径实例**（腾出配额再播新）。
    ///
    /// LN 持续音事件密集触发同一 wav：若放任叠加会盖过 BGM；停旧播新让
    /// 发声连续且每次事件都出声，同时该路径并发数恒 ≤ [`KEYSOUND_MAX_PER_PATH`]。
    fn keysound_make_room(&mut self, path: &Path) {
        while self.keysound_limiter.count(path) >= KEYSOUND_MAX_PER_PATH {
            let Some(pos) = self
                .keysound_voices
                .iter()
                .position(|v| v.path() == path)
            else {
                break;
            };
            let mut voice = self.keysound_voices.remove(pos);
            voice.stop();
            self.keysound_limiter.release(voice.path());
        }
    }

    /// 播放一次 BGM 事件（`ChartEvent::Bgm` 触发）。
    ///
    /// 按采样角色分两种语义（见 [`is_bed_audio`] / [`bgm_event_action`]）：
    /// - **连续音频 Bed**（时长 ≥ 铺面总时长 × [`BED_MIN_CHART_RATIO`]）：去重。
    ///   BMS 谱面常在 01 通道反复放置同一整曲 BGM，若每个事件都从头重播，BGM
    ///   永远跟不上谱面；相同路径仍在播放 → 忽略，换路径 → 叠加开始新的。
    /// - **短采样**（鼓组/打击乐）：**每个事件都发声**，走一次性采样轨
    ///   （`bgm_sample`），同时发声数受 [`BGM_SAMPLE_MAX_VOICES`] 软上限约束
    ///   （超出停最旧，保证本次必发声）。
    ///
    /// 旧实现把全部静态 BGM 都塞进键音轨且不计数：密集谱面会撑满 kira 子轨
    /// 默认的 128 发声上限 → `SoundLimitReached` → 之后的事件**全部丢音**
    /// （2026-10 用户报告 MORNINGLOOM 鼓组 BGM 连续丢音）。
    pub fn play_bgm_event(&mut self, path: &Path) -> bool {
        self.ensure_song_tracks();
        // 流式长 BGM：不与一次性采样混轨，去重后走 `play_bgm`
        if !self.should_cache(path) {
            if self.bgm_path.as_deref() == Some(path)
                && self.bgm_handle.as_ref().is_some_and(BgmHandle::is_playing)
            {
                return true; // 相同 BGM 仍在播放 → 忽略（不重播）
            }
            self.bgm_path = Some(path.to_path_buf());
            // 失败回滚 bgm_path，避免去重误判"在播"
            let ok = self.play_bgm(path);
            if !ok {
                self.bgm_path = None;
            }
            return ok;
        }
        // 静态缓存：priority 预解码命中直接播；未命中同步解码一次并缓存
        let data = match self.cache.get(path) {
            Some(data) => data.clone(),
            None => match StaticSoundData::from_file(path) {
                Ok(data) => {
                    self.cache.insert_ready(path.to_path_buf(), data.clone());
                    data
                }
                Err(e) => {
                    warn!("[audio] BGM 同步解码失败 {}: {e}", path.display());
                    self.bgm_path = None; // 失败回滚，避免去重误判"在播"
                    return false;
                }
            },
        };
        let is_bed = self.bgm_role(path, data.duration().as_secs_f64());
        let playing_bgm = self.bgm_handle.as_ref().is_some_and(BgmHandle::is_playing);
        let action = bgm_event_action(
            is_bed,
            self.bgm_path.as_deref().map(|p| (p, playing_bgm)),
            path,
        );
        match action {
            BgmEventAction::Ignore => true,
            BgmEventAction::Start => self.start_long_bgm(path, data),
            BgmEventAction::OneShot => self.play_bgm_sample(data),
        }
    }

    /// 查询（并首次计算）该 BGM 路径的角色：`true` = 连续音频（Bed）。
    ///
    /// 判定 `is_bed_audio(duration, song_total_secs)` 只在首次遇到该路径时
    /// 计算并 `debug!` 打一行（谱面音色/时长误判可直接从日志定位），
    /// 之后走缓存；每场铺面退出时缓存清空。
    fn bgm_role(&mut self, path: &Path, duration_secs: f64) -> bool {
        if let Some(&is_bed) = self.bgm_roles.get(path) {
            return is_bed;
        }
        let is_bed = self.song_total_known && is_bed_audio(duration_secs, self.song_total_secs);
        // Bed 很少（一谱面 0–2 条）→ info 便于试听时核对判定；采样可能上百条 → debug
        if is_bed {
            info!(
                "[audio] BGM 角色 {}: duration={duration_secs:.2}s / chart={:.2}s → Bed（同路径在播则忽略）",
                path.display(),
                self.song_total_secs,
            );
        } else {
            debug!(
                "[audio] BGM 角色 {}: duration={duration_secs:.2}s / chart={:.2}s → OneShot（每事件发声）",
                path.display(),
                self.song_total_secs,
            );
        }
        self.bgm_roles.insert(path.to_path_buf(), is_bed);
        is_bed
    }

    /// 开始在 BGM 轨上播放长 BGM（叠加语义：不打断其它正在播的 BGM）。
    ///
    /// `bgm_handle`/`bgm_path` 只保留**最近**一条，供同路径去重使用；
    /// 更早的实例自然播完，或随 `stop_all` 销毁轨道一起停止。
    fn start_long_bgm(&mut self, path: &Path, data: StaticSoundData) -> bool {
        let Some(bgm_track) = self.tracks.bgm.as_mut() else {
            self.bgm_path = None;
            return false;
        };
        match bgm_track.play(data) {
            Ok(handle) => {
                self.bgm_handle = Some(BgmHandle::Static(handle));
                self.bgm_path = Some(path.to_path_buf());
                true
            }
            Err(e) => {
                warn!("[audio] BGM 静态播放失败 {}: {e}", path.display());
                self.bgm_path = None; // 失败回滚，避免去重误判"在播"
                false
            }
        }
    }

    /// 播放一次性 BGM 采样：清理已结束实例，超出软上限时停最旧，保证本次发声。
    fn play_bgm_sample(&mut self, data: StaticSoundData) -> bool {
        self.prune_bgm_samples();
        while self.bgm_samples.len() >= BGM_SAMPLE_MAX_VOICES {
            let mut oldest = self.bgm_samples.remove(0);
            oldest.stop(Tween::default());
        }
        let Some(sample_track) = self.tracks.bgm_sample.as_mut() else {
            return false;
        };
        match sample_track.play(data) {
            Ok(handle) => {
                self.bgm_samples.push(handle);
                true
            }
            Err(e) => {
                warn!("[audio] BGM 采样播放失败: {e}");
                false
            }
        }
    }

    /// 清理一次性采样轨上已结束的实例（防止句柄列表无界增长）。
    fn prune_bgm_samples(&mut self) {
        self.bgm_samples
            .retain(|handle| handle.state() == PlaybackState::Playing);
    }

    /// 播放 BGM：流式解码 + 谱面时钟对齐（音频线程精确开始）。
    ///
    /// BMS 的每个 BGM 事件相互独立：同一路径仍在播时由调用方去重，不同路径
    /// 叠加播放（不打断）。句柄统一由 `stop_all`/轨道销毁停止。
    fn play_bgm(&mut self, path: &Path) -> bool {
        let Some(bgm_track) = self.tracks.bgm.as_mut() else {
            return false;
        };
        let clock_now = self.clock.time();
        match StreamingSoundData::from_file(path).map(|d| d.start_time(clock_now)) {
            Ok(data) => match bgm_track.play(data) {
                Ok(handle) => {
                    self.bgm_handle = Some(BgmHandle::Streaming(handle));
                    true
                }
                Err(e) => {
                    warn!("[audio] BGM 播放失败 {}: {e}", path.display());
                    false
                }
            },
            Err(e) => {
                warn!("[audio] BGM 解码失败 {}: {e}", path.display());
                false
            }
        }
    }

    /// 播放主界面（选曲/标题）BGM：流式解码，切换时先停旧曲。
    ///
    /// 独立于 gameplay 音轨：`stop_all` 不会碰它，生命周期由选曲界面控制
    /// （进入游玩/退出选曲时 `stop_menu_bgm`）。
    /// 预留：当前无主界面音乐源配置，接入（如设置项指定选曲 BGM 路径）后调用。
    #[allow(dead_code)]
    pub fn play_menu_bgm(&mut self, path: &Path) -> bool {
        self.stop_menu_bgm();
        match StreamingSoundData::from_file(path) {
            Ok(data) => match self.tracks.menu.play(data) {
                Ok(handle) => {
                    self.menu_handle = Some(handle);
                    true
                }
                Err(e) => {
                    warn!("[audio] 主界面 BGM 播放失败 {}: {e}", path.display());
                    false
                }
            },
            Err(e) => {
                warn!("[audio] 主界面 BGM 解码失败 {}: {e}", path.display());
                false
            }
        }
    }

    /// 停止主界面 BGM（进入游玩 / 退出选曲时调用）。
    pub fn stop_menu_bgm(&mut self) {
        if let Some(mut handle) = self.menu_handle.take() {
            handle.stop(Tween::default());
        }
    }

    /// 该路径是否走静态缓存播放（而非 BGM 流式）。
    ///
    /// 注意：若同一 wav 同时被 BGM 事件与键音事件引用，BGM 标记优先——
    /// 键音事件会走流式分支（每次新开流），失去即时性；BMS 中此情况罕见，
    /// 保持与 beatoraja 一致的"BGM 通道优先"语义。
    fn should_cache(&self, path: &Path) -> bool {
        !is_streaming_path(&self.streaming_paths, path)
    }

    /// 停止并**销毁**所有正在播放的 gameplay 音频（退出谱面时调用）：
    /// drop 本场谱面的播放轨道 → 轨道上的全部声音（大 BGM 流 + 静态 BGM + 键音）
    /// 随之停止；暂停谱面时钟。
    ///
    /// 注意：不用 `pause`——pause 只暂停不销毁，下一场 `resume` 会让旧声音
    /// 复活重叠（2026-08 修复的 bug）。不影响主界面音轨（`menu_track`）。
    pub fn stop_all(&mut self) {
        // 销毁本场铺面轨道：`TrackHandle` drop 触发 kira 轨道移除（`mark_for_removal`），
        // 轨道默认 `persist_until_sounds_finish=false`，下一音频回调周期即整体销毁，
        // 其上所有声音（大 BGM 流 + 静态 BGM + 键音）随之停止，无"播完才停"残留。
        self.tracks.destroy_song_tracks();
        if let Some(mut handle) = self.bgm_handle.take() {
            handle.stop();
        }
        self.bgm_path = None;
        // 本场键音/一次性 BGM 采样随轨道销毁全部停止，同步清空计数与句柄列表
        self.keysound_voices.clear();
        self.keysound_limiter.clear();
        self.bgm_samples.clear();
        self.bgm_roles.clear();
        self.clock.pause();
    }

    /// 是否还有音频在播放（用于完成谱面时等待背景音乐自然结束）。
    ///
    /// 检查本场谱面轨道：长 BGM 轨 + 一次性 BGM 采样轨 + 键音轨
    /// （`num_sounds` 不受常驻节拍器影响——节拍器在独立轨道）。
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.bgm_handle
            .as_ref()
            .is_some_and(BgmHandle::is_playing)
            || self.tracks.bgm.as_ref().is_some_and(|t| t.num_sounds() > 0)
            || self
                .tracks
                .bgm_sample
                .as_ref()
                .is_some_and(|t| t.num_sounds() > 0)
            || self
                .tracks
                .keysound
                .as_ref()
                .is_some_and(|t| t.num_sounds() > 0)
    }

    /// 设置全局音量（0.0–1.0，线性值 → main track 的 dB，`Tween` 平滑过渡）。
    pub fn set_volume(&mut self, volume: f32) {
        let volume = volume.clamp(0.0, 1.0);
        if (self.volume - volume).abs() < 1e-4 {
            return;
        }
        self.volume = volume;
        self.kira
            .main_track()
            .set_volume(linear_to_db(volume), Tween::default());
    }

    /// 控制节拍器（第 6 章自定义 Sound）：拍速 + 开关。
    /// 预留：后续接入设置/键位（如练习模式节拍器）。
    #[allow(dead_code)]
    pub fn set_metronome(&mut self, tempo: f64, enabled: bool) {
        self.metronome.set_tempo(tempo);
        self.metronome.set_enabled(enabled);
        info!(
            "[audio] 节拍器 {}（{tempo} BPM）",
            if enabled { "开启" } else { "关闭" }
        );
    }

    /// 是否已请求的音频全部就绪（首批语义见 [`AudioCache`]）。
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.cache.is_ready()
    }

    /// 加载进度（已就绪 / 已请求；硬编码 HUD 移除后暂无调用，保留供后续 UI）。
    #[must_use]
    #[allow(dead_code)]
    pub fn progress(&self) -> (usize, usize) {
        self.cache.progress()
    }
}

/// 路径是否已被标记为 BGM（流式播放，不缓存）。
fn is_streaming_path(streaming: &HashSet<PathBuf>, path: &Path) -> bool {
    streaming.contains(path)
}

/// 键音轨上的一个播放中实例（静态缓存或流式大文件），统一并发管理。
enum KeysoundVoice {
    /// 静态缓存采样（普通键音/小 BGM 采样）。
    Static { path: PathBuf, handle: StaticSoundHandle },
    /// 流式大文件（≥2MB 且被标记流式 BGM，键音事件引用时流式播放）。
    Streaming {
        path: PathBuf,
        handle: StreamingSoundHandle<FromFileError>,
    },
}

impl KeysoundVoice {
    fn path(&self) -> &Path {
        match self {
            Self::Static { path, .. } | Self::Streaming { path, .. } => path,
        }
    }

    /// 是否仍在播放（已播完/被停止 = 结束）。
    fn is_playing(&self) -> bool {
        match self {
            Self::Static { handle, .. } => handle.state() == PlaybackState::Playing,
            Self::Streaming { handle, .. } => handle.state() == PlaybackState::Playing,
        }
    }

    /// 停止播放（10ms 渐隐）。
    fn stop(&mut self) {
        match self {
            Self::Static { handle, .. } => handle.stop(Tween::default()),
            Self::Streaming { handle, .. } => handle.stop(Tween::default()),
        }
    }
}

/// 键音并发限制器（纯逻辑，无设备依赖，可单测）。
///
/// 维护「路径 → 正在播放的实例数」，配合 [`AudioManager::keysound_voices`]
/// 实现同路径/总量双重上限。实例播完由 `prune` 归还计数，轨道销毁由
/// [`Self::clear`] 整体重置。
#[derive(Debug, Default)]
struct KeysoundLimiter {
    counts: HashMap<PathBuf, usize>,
}

impl KeysoundLimiter {
    /// 该路径当前并发实例数。
    fn count(&self, path: &Path) -> usize {
        self.counts.get(path).copied().unwrap_or(0)
    }

    /// 尝试占用一个并发配额：总实例数达上限时拒绝（返回 `false`），
    /// 否则计数 +1 并返回 `true`。
    fn try_acquire(&mut self, path: &Path, max_total: usize) -> bool {
        let total: usize = self.counts.values().sum();
        if total >= max_total {
            return false;
        }
        *self.counts.entry(path.to_path_buf()).or_insert(0) += 1;
        true
    }

    /// 归还一个并发配额（实例已结束/被停止）。
    fn release(&mut self, path: &Path) {
        if let Some(n) = self.counts.get_mut(path) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.counts.remove(path);
            }
        }
    }

    /// 清空全部计数（本场铺面轨道销毁时调用）。
    fn clear(&mut self) {
        self.counts.clear();
    }
}

/// 文件是否应走流式解码（真长 BGM）：按磁盘大小判定，文件不存在
/// 或过小时返回 `false`（走静态缓存，多路并发）。
fn should_stream_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.len() >= STREAMING_MIN_BYTES)
        .unwrap_or(false)
}

/// 组合判定：文件大小 + 事件稀疏度（`register_bgm` 的分流决策）。
fn should_stream_file_with_events(path: &Path, events: usize) -> bool {
    should_stream_file(path) && events <= STREAMING_MAX_EVENTS
}

/// 取出预加载时间线中**窗口内到期**的路径，并推进游标（纯逻辑，可单测）。
///
/// 时间线按首触发时间升序；`now_sec + lookahead` 之外的路径留待后续帧。
fn due_preload_paths(
    timeline: &[(f64, PathBuf)],
    idx: &mut usize,
    now_sec: f64,
    lookahead: f64,
) -> Vec<PathBuf> {
    let deadline = now_sec + lookahead;
    let mut due = Vec::new();
    while let Some((t, path)) = timeline.get(*idx) {
        if *t > deadline {
            break;
        }
        due.push(path.clone());
        *idx += 1;
    }
    due
}

/// BGM 事件的处理动作（纯逻辑，可单测）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BgmEventAction {
    /// 同一条长 BGM 仍在播放 → 忽略（不从头重播）。
    Ignore,
    /// 长 BGM：从头播放（**叠加**，不停止其它正在播的长 BGM——
    /// BMS 中每个 BGM 事件相互独立，互不打断）。
    Start,
    /// 短采样：每次都发声，与当前 BGM 无关。
    OneShot,
}

/// 连续音频（Bed）判据：采样时长 ≥ 铺面总时长 × [`BED_MIN_CHART_RATIO`]。
///
/// 分母未知（≤ 0）时一律返回 `false` → 按采样处理，宁可叠音不可吞音。
fn is_bed_audio(duration_secs: f64, chart_total_secs: f64) -> bool {
    chart_total_secs > 0.0 && duration_secs >= chart_total_secs * BED_MIN_CHART_RATIO
}

/// 依据采样角色与当前 BGM 状态决定事件动作。
///
/// `current` = (当前记录的 BGM 路径, 是否仍在播放)。采样（`is_bed == false`）
/// 永远 `OneShot`——去重会让密集滚奏被削薄（丢音）。
fn bgm_event_action(
    is_bed: bool,
    current: Option<(&Path, bool)>,
    path: &Path,
) -> BgmEventAction {
    if !is_bed {
        return BgmEventAction::OneShot;
    }
    match current {
        Some((playing_path, true)) if playing_path == path => BgmEventAction::Ignore,
        _ => BgmEventAction::Start,
    }
}

/// 线性音量（0.0–1.0）→ kira 分贝值。0 静音映射到 `Decibels::SILENCE`。
fn linear_to_db(linear: f32) -> Decibels {
    if linear <= f32::EPSILON {
        Decibels::SILENCE
    } else {
        Decibels(20.0 * linear.log10())
    }
}

/// 谱面持有的音频租约（记录占用的路径，退出时交还 `AudioManager`）。
#[derive(Debug, Default)]
pub struct AudioLease {
    paths: Vec<PathBuf>,
}

/// 音频缓存：引用计数 + LRU 淘汰 + 加载进度（纯逻辑，与设备/线程解耦）。
///
/// 缓存对象为 kira `StaticSoundData`（采样 `Arc` 共享，克隆零拷贝）。
#[derive(Default)]
struct AudioCache {
    /// 已就绪缓存：path → 静态声音数据。
    cache: HashMap<PathBuf, StaticSoundData>,
    /// 引用计数：path → 持有租约数。
    refs: HashMap<PathBuf, usize>,
    /// path → 解码后采样字节数（LRU 按字节淘汰用）。
    bytes: HashMap<PathBuf, u64>,
    /// 最近使用顺序（末尾最新，用于 LRU 淘汰）。
    lru: Vec<PathBuf>,
    /// 已请求加载的路径集合。
    #[allow(dead_code)] // 与 priority_set 一起构成加载进度语义
    requested: HashSet<PathBuf>,
    /// 首批（开玩必需）路径集合：就绪即视为可开始游玩。
    priority_set: HashSet<PathBuf>,
}

impl AudioCache {
    fn acquire(&mut self, paths: &[PathBuf]) -> AudioLease {
        for path in paths {
            *self.refs.entry(path.clone()).or_insert(0) += 1;
            self.requested.insert(path.clone());
        }
        AudioLease {
            paths: paths.to_vec(),
        }
    }

    fn release(&mut self, lease: &AudioLease) {
        for path in &lease.paths {
            if let Some(n) = self.refs.get_mut(path) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    // 引用归零 → **立即释放解码数据**（铺面退出即回收内存，
                    // 不等 LRU 上限），并清理全部簿记。
                    self.refs.remove(path);
                    self.cache.remove(path);
                    self.bytes.remove(path);
                    self.lru.retain(|p| p != path);
                    self.requested.remove(path);
                    self.priority_set.remove(path);
                }
            }
        }
        self.evict_if_needed();
    }

    /// 收拢一个解码完成的结果，更新 LRU 顺序并尝试淘汰。
    fn insert_ready(&mut self, path: PathBuf, data: StaticSoundData) {
        // 引用已释放（谱面退出后到达的后台解码结果）：不缓存——
        // 否则会以 refs=0 永久残留，造成内存泄漏。
        if !self.refs.contains_key(&path) {
            return;
        }
        let size = (data.frames.len() as u64) * std::mem::size_of::<Frame>() as u64;
        self.bytes.insert(path.clone(), size);
        self.cache.insert(path.clone(), data);
        self.lru.retain(|p| p != &path);
        self.lru.push(path);
        self.evict_if_needed();
    }

    /// 记录一个加载失败的路径（从待加载集合移除，避免永久等待）。
    fn remove_requested(&mut self, path: &Path) {
        self.requested.remove(path);
        self.priority_set.remove(path);
    }

    /// 标记路径为首批（开玩必需）。
    fn mark_priority(&mut self, path: &Path) {
        self.priority_set.insert(path.to_path_buf());
    }

    fn get(&self, path: &Path) -> Option<StaticSoundData> {
        self.cache.get(path).cloned()
    }

    /// 首批是否全部就绪（就绪即可开始游玩）。
    fn is_ready(&self) -> bool {
        self.priority_set
            .iter()
            .all(|p| self.cache.contains_key(p))
    }

    fn progress(&self) -> (usize, usize) {
        let ready = self
            .priority_set
            .iter()
            .filter(|p| self.cache.contains_key(*p))
            .count();
        (ready, self.priority_set.len())
    }

    /// LRU 淘汰：移除最旧的**非首批**项，直到缓存不超限（数量 + 字节双上限）。
    ///
    /// **为什么不是"未引用项"**：谱面进入时 `acquire` 会给该谱面**全部**音频路径
    /// 加租约（用于退出时一键释放），因此游玩期间缓存项几乎全部 `refs > 0`。
    /// 旧实现只在 `refs == 0` 里找淘汰候选 → 找不到就 `break`，数量/字节上限
    /// **从未生效**，缓存会一直涨到整张谱面解码后的总量（数千个键音 = 数百 MB，
    /// 即"加载铺面内存占用过大"）。
    ///
    /// 现在只保留首批（`submit_priority` 标记、开玩必需）为 pinned，其余可淘汰；
    /// 被淘汰项再次触发时 `play_synced` / `play_bgm_event` 会同步解码补回。
    fn evict_if_needed(&mut self) {
        loop {
            let over_count = self.cache.len() > LRU_CACHE_MAX;
            let over_bytes = self.bytes.values().sum::<u64>() > AUDIO_CACHE_MAX_BYTES;
            if !over_count && !over_bytes {
                return;
            }
            let Some(pos) = self
                .lru
                .iter()
                .position(|p| !self.priority_set.contains(p))
            else {
                return; // 全部首批项：无法再压（首批量本身很小）
            };
            let victim = self.lru.remove(pos);
            self.cache.remove(&victim);
            self.bytes.remove(&victim);
            // 注意：不清 `refs`（租约语义是"谱面作用域"，退出时统一释放）；
            // 也不清 `requested`（已请求解码的簿记，避免重复登记）。
        }
    }
}

/// 音频管理插件：初始化管理器 + 每帧同步音量设置。
pub struct AudioManagerPlugin;

impl Plugin for AudioManagerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init_audio_manager)
            .add_systems(Update, (sync_volume, drain_audio_manager));
    }
}

fn init_audio_manager(mut commands: Commands) {
    match AudioManager::new() {
        Ok(manager) => {
            info!("[audio] 音频系统就绪（kira/cpal 后端，内部缓冲 128 采样）");
            commands.insert_resource(manager);
        }
        Err(e) => error!("[audio] 初始化失败: {e}"),
    }
}

/// 每帧把设置里的全局音量同步到 main track（变化时才下发，避免刷命令）。
fn sync_volume(mut manager: ResMut<AudioManager>, store: Res<SettingsStore>) {
    let volume = store.get_float("volume", 1.0) as f32;
    manager.set_volume(volume);
}

/// 每帧收拢后台解码结果（非阻塞，渐进预加载入缓存）+ 清理已结束的键音实例。
fn drain_audio_manager(mut manager: ResMut<AudioManager>) {
    manager.drain_ready();
    manager.prune_keysound_voices();
}

#[cfg(test)]
mod tests {
    use super::*;
    use kira::{
        Frame,
        sound::static_sound::StaticSoundSettings,
    };

    /// 构造一段静态声音数据（不依赖真实文件）。
    fn static_data(samples: usize) -> StaticSoundData {
        StaticSoundData {
            sample_rate: 44_100,
            frames: vec![Frame::ZERO; samples].into(),
            settings: StaticSoundSettings::default(),
            slice: None,
        }
    }

    #[test]
    fn keysound_limiter_per_path_count_and_release() {
        let mut l = KeysoundLimiter::default();
        let a = PathBuf::from("/ln.wav");
        let b = PathBuf::from("/tap.wav");

        // 同路径多次 acquire：计数累加
        assert!(l.try_acquire(&a, KEYSOUND_MAX_TOTAL));
        assert!(l.try_acquire(&a, KEYSOUND_MAX_TOTAL));
        assert_eq!(l.count(&a), 2);
        assert_eq!(l.count(&b), 0);

        // release 归还计数，归零后移除
        l.release(&a);
        assert_eq!(l.count(&a), 1);
        l.release(&a);
        assert_eq!(l.count(&a), 0);
    }

    #[test]
    fn keysound_limiter_total_cap_rejects_and_recovers() {
        let mut l = KeysoundLimiter::default();
        // 占满总量（不同路径）
        for i in 0..KEYSOUND_MAX_TOTAL {
            assert!(
                l.try_acquire(&PathBuf::from(format!("/k{i}.wav")), KEYSOUND_MAX_TOTAL),
                "第 {i} 个应占用成功"
            );
        }
        // 超限拒绝
        assert!(!l.try_acquire(&PathBuf::from("/overflow.wav"), KEYSOUND_MAX_TOTAL));
        // 释放一个后恢复
        l.release(&PathBuf::from("/k0.wav"));
        assert!(l.try_acquire(&PathBuf::from("/overflow.wav"), KEYSOUND_MAX_TOTAL));
    }

    #[test]
    fn keysound_limiter_clear_resets_all() {
        let mut l = KeysoundLimiter::default();
        l.try_acquire(&PathBuf::from("/a.wav"), KEYSOUND_MAX_TOTAL);
        l.try_acquire(&PathBuf::from("/b.wav"), KEYSOUND_MAX_TOTAL);
        l.clear();
        assert_eq!(l.count(&PathBuf::from("/a.wav")), 0);
        assert_eq!(l.count(&PathBuf::from("/b.wav")), 0);
        assert!(l.try_acquire(&PathBuf::from("/a.wav"), KEYSOUND_MAX_TOTAL));
    }

    #[test]
    fn bgm_event_action_by_chart_ratio() {
        let bed_a = PathBuf::from("/bgm/song_full.ogg");
        let bed_b = PathBuf::from("/bgm/song_alt.ogg");
        let hat = PathBuf::from("/samples/hat.ogg");

        // 判据本身：相对铺面时长，无绝对秒数
        assert!(!is_bed_audio(1.32, 136.0), "1.32s 鼓组在 136s 铺面里是采样");
        assert!(is_bed_audio(180.0, 136.0), "整曲 BGM 比铺面还长 → Bed");
        assert!(is_bed_audio(34.0, 136.0), "达到 25% 铺面时长即 Bed");
        assert!(!is_bed_audio(33.9, 136.0), "差一点不到 → 采样");
        assert!(!is_bed_audio(180.0, 0.0), "铺面时长未知 → 按采样（不吞音）");

        // 采样：无论当前在播什么都必须发声（密集滚奏不能被去重削薄）
        assert_eq!(
            bgm_event_action(false, Some((&hat, true)), &hat),
            BgmEventAction::OneShot,
            "采样即使同路径在播也应重触发"
        );
        // MORNINGLOOM 实测：1.02–1.32s 的长尾鼓组采样在 136s 铺面里仍是采样，
        // 此前用 1.0s 绝对阈值把 42% 鼓组事件判成 Bed 而丢弃
        assert_eq!(
            bgm_event_action(is_bed_audio(1.32, 136.0), Some((&hat, true)), &hat),
            BgmEventAction::OneShot
        );

        // Bed：同路径仍在播 → 忽略（避免整曲从头重播）
        assert_eq!(
            bgm_event_action(true, Some((&bed_a, true)), &bed_a),
            BgmEventAction::Ignore
        );
        // Bed：同路径但已播完 → 从头重播
        assert_eq!(
            bgm_event_action(true, Some((&bed_a, false)), &bed_a),
            BgmEventAction::Start
        );
        // Bed：换曲 → 叠加开始新曲（不打断仍在播的其它 BGM）
        assert_eq!(
            bgm_event_action(true, Some((&bed_a, true)), &bed_b),
            BgmEventAction::Start
        );
        // Bed：当前无 BGM → 直接开始
        assert_eq!(bgm_event_action(true, None, &bed_a), BgmEventAction::Start);
    }

    #[test]
    fn bed_ratio_depends_on_song_length() {
        // 同一采样在不同铺面里角色可以不同：判据是"占铺面多长"，不是绝对秒数。
        let dur = 60.0;
        assert!(is_bed_audio(dur, 136.0), "60s 占 136s 铺面 44% → Bed");
        assert!(
            !is_bed_audio(dur, 480.0),
            "同样 60s 在 8 分钟铺面里只占 12.5% → 采样"
        );
        assert!(!is_bed_audio(dur, 0.0), "铺面时长未知 → 采样（不吞音）");
    }

    #[test]
    fn cache_refcount_and_eviction() {
        let mut c = AudioCache::default();
        let a = PathBuf::from("/a.ogg");
        let b = PathBuf::from("/b.ogg");

        // acquire 两次 → refs=2
        let l1 = c.acquire(std::slice::from_ref(&a));
        let l2 = c.acquire(&[a.clone(), b.clone()]);
        assert_eq!(c.refs.get(&a), Some(&2));
        assert_eq!(c.refs.get(&b), Some(&1));

        // 就绪后 is_ready（首批：标记 a/b 为 priority）
        c.mark_priority(&a);
        c.mark_priority(&b);
        c.insert_ready(a.clone(), static_data(8));
        c.insert_ready(b.clone(), static_data(8));
        assert!(c.is_ready());
        assert_eq!(c.progress(), (2, 2));
        assert!(c.get(&a).is_some());

        // 释放一次 → refs=1，仍保留
        c.release(&l1);
        assert_eq!(c.refs.get(&a), Some(&1));
        assert!(c.get(&a).is_some());

        // 全部释放 → 引用归零**立即移除**解码数据（铺面退出即回收内存）
        c.release(&l2);
        assert_eq!(c.refs.get(&a), None);
        assert!(c.get(&a).is_none(), "引用归零应立即释放");
        assert!(c.get(&b).is_none());
    }

    #[test]
    fn release_frees_cache_immediately() {
        let mut c = AudioCache::default();
        let a = PathBuf::from("/a.ogg");
        let lease = c.acquire(std::slice::from_ref(&a));
        c.insert_ready(a.clone(), static_data(100));
        assert!(c.cache.contains_key(&a));
        assert!(c.bytes.contains_key(&a));
        c.release(&lease);
        assert!(!c.cache.contains_key(&a), "铺面退出应立即释放解码数据");
        assert!(!c.bytes.contains_key(&a));
        assert!(c.lru.is_empty());
    }

    #[test]
    fn insert_ready_after_release_is_dropped() {
        let mut c = AudioCache::default();
        let a = PathBuf::from("/a.ogg");
        let lease = c.acquire(std::slice::from_ref(&a));
        // 退出后（引用已释放）才到达的后台解码结果不应重新入缓存
        c.release(&lease);
        c.insert_ready(a.clone(), static_data(100));
        assert!(!c.cache.contains_key(&a), "已释放路径的解码结果不应缓存");
    }

    #[test]
    fn lru_evicts_unreferenced() {
        let mut c = AudioCache::default();
        // 直接注入**未引用**项（绕过 insert_ready 的 refs 检查），
        // 验证 evict_if_needed 的数量 + 字节双上限淘汰最旧项。
        for i in 0..(LRU_CACHE_MAX + 4) {
            let p = PathBuf::from(format!("/x{i}.ogg"));
            c.cache.insert(p.clone(), static_data(8));
            c.bytes.insert(p.clone(), (8 * std::mem::size_of::<Frame>()) as u64);
            c.lru.push(p);
        }
        c.evict_if_needed();
        assert!(c.cache.len() <= LRU_CACHE_MAX, "len={}", c.cache.len());
        assert!(c.get(&PathBuf::from("/x0.ogg")).is_none(), "最旧应被淘汰");
        assert!(
            c.get(&PathBuf::from(format!("/x{}.ogg", LRU_CACHE_MAX + 3)))
                .is_some(),
            "最新应保留"
        );
    }

    #[test]
    fn evict_by_bytes_cap() {
        let mut c = AudioCache::default();
        // 单项体积 = 字节上限的 60%（两项 = 120% > 上限）→ 按 LRU 淘汰最旧
        let big = ((AUDIO_CACHE_MAX_BYTES as usize) / std::mem::size_of::<Frame>()) * 3 / 5;
        let a = PathBuf::from("/big_a.ogg");
        let b = PathBuf::from("/big_b.ogg");
        c.cache.insert(a.clone(), static_data(big));
        c.bytes.insert(a.clone(), (big * std::mem::size_of::<Frame>()) as u64);
        c.lru.push(a.clone());
        c.cache.insert(b.clone(), static_data(big));
        c.bytes.insert(b.clone(), (big * std::mem::size_of::<Frame>()) as u64);
        c.lru.push(b.clone());
        c.evict_if_needed();
        assert!(!c.cache.contains_key(&a), "字节超限应淘汰最旧未引用项");
        assert!(c.cache.contains_key(&b), "最新项保留");
    }

    /// 被谱面租约引用（`refs > 0`）但**非首批**的项也必须可淘汰。
    ///
    /// 这是"加载铺面内存占用过大"的根因回归测试：旧实现只在 `refs == 0` 中找
    /// 淘汰候选，而谱面进入时所有路径都有租约 → 上限从未生效，缓存涨到整张
    /// 谱面解码后的总量。
    #[test]
    fn evict_referenced_but_unpinned_entries() {
        let mut c = AudioCache::default();
        let pinned = PathBuf::from("/pinned.ogg");
        let loose_a = PathBuf::from("/loose_a.ogg");
        let loose_b = PathBuf::from("/loose_b.ogg");
        // 三项都有租约（谱面作用域），只有 pinned 被首批标记
        let _lease = c.acquire(&[pinned.clone(), loose_a.clone(), loose_b.clone()]);
        c.mark_priority(&pinned);
        // 账面体积 = 上限的 60%（不真分配采样，只验证淘汰决策）
        let per_item = AUDIO_CACHE_MAX_BYTES * 3 / 5;
        for p in [&pinned, &loose_a, &loose_b] {
            c.cache.insert(p.clone(), static_data(8));
            c.bytes.insert(p.clone(), per_item);
            c.lru.push(p.clone());
        }
        c.evict_if_needed();
        assert!(c.get(&pinned).is_some(), "首批项 pinned，不可淘汰");
        assert_eq!(c.cache.len(), 1, "非首批项应被淘汰到不超上限");
        assert!(c.get(&loose_a).is_none() && c.get(&loose_b).is_none());
        assert!(
            c.refs.contains_key(&loose_a) && c.refs.contains_key(&loose_b),
            "淘汰不应影响租约簿记（退出时统一释放）"
        );
        assert!(c.bytes.values().sum::<u64>() <= AUDIO_CACHE_MAX_BYTES);
    }

    /// 预加载滚动窗口：只取 `now + lookahead` 内的路径，游标只前进不回头。
    #[test]
    fn preload_window_respects_lookahead() {
        let timeline = vec![
            (0.0, PathBuf::from("/a.ogg")),
            (5.0, PathBuf::from("/b.ogg")),
            (12.0, PathBuf::from("/c.ogg")),
            (30.0, PathBuf::from("/d.ogg")),
        ];
        let mut idx = 0;
        let first = due_preload_paths(&timeline, &mut idx, 0.0, PRELOAD_LOOKAHEAD_SECS);
        assert_eq!(first, vec![PathBuf::from("/a.ogg"), PathBuf::from("/b.ogg")]);
        assert_eq!(idx, 2);
        // 同一时刻重复推进：不重复提交
        assert!(due_preload_paths(&timeline, &mut idx, 0.0, PRELOAD_LOOKAHEAD_SECS).is_empty());
        // 时间推进到 4s：窗口覆盖 14s → c 到期
        let second = due_preload_paths(&timeline, &mut idx, 4.0, PRELOAD_LOOKAHEAD_SECS);
        assert_eq!(second, vec![PathBuf::from("/c.ogg")]);
        // d 在 30s，仍需等时间推进
        assert!(due_preload_paths(&timeline, &mut idx, 10.0, PRELOAD_LOOKAHEAD_SECS).is_empty());
        let last = due_preload_paths(&timeline, &mut idx, 21.0, PRELOAD_LOOKAHEAD_SECS);
        assert_eq!(last, vec![PathBuf::from("/d.ogg")]);
        assert_eq!(idx, timeline.len());
    }

    /// 线性音量 → dB 换算（main track 音量映射）。
    #[test]
    fn linear_volume_to_decibels() {
        assert_eq!(linear_to_db(1.0), Decibels(0.0));
        assert_eq!(linear_to_db(0.0), Decibels::SILENCE);
        // 0.5 → 20·log10(0.5) ≈ -6.02 dB
        let db = linear_to_db(0.5);
        assert!((db.0 - (-6.0206)).abs() < 1e-3, "db={}", db.0);
        // 单调：音量越小 dB 越低
        assert!(linear_to_db(0.2).0 < linear_to_db(0.8).0);
    }

    /// 真实音频解码（kira `StaticSoundData::from_file`；文件不存在时跳过）。
    #[test]
    fn bgm_paths_skip_cache() {
        let bgm = PathBuf::from("/bgm/loop.ogg");
        let keysound = PathBuf::from("/keys/note01.wav");
        let mut streaming = HashSet::new();
        streaming.insert(bgm.clone());
        assert!(!should_cache_with(&streaming, &bgm), "BGM 不应进缓存");
        assert!(should_cache_with(&streaming, &keysound), "键音应进缓存");
    }

    /// [`should_cache`] 的判定（AudioManager 需真音频设备无法构造，
    /// 用提取的纯函数等价验证）。
    fn should_cache_with(streaming: &HashSet<PathBuf>, path: &Path) -> bool {
        !is_streaming_path(streaming, path)
    }

    /// 大小 + 事件数分流：大文件且稀疏引用走流式，其余走静态缓存。
    #[test]
    fn streaming_by_file_size() {
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("rxbms-audio-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        let big = dir.join("big.ogg");
        let small = dir.join("small.ogg");
        let missing = dir.join("missing.ogg");

        let mut f = std::fs::File::create(&big).expect("创建大文件");
        f.write_all(&vec![0u8; STREAMING_MIN_BYTES as usize]).expect("写入大文件");
        let mut f = std::fs::File::create(&small).expect("创建小文件");
        f.write_all(&[0u8; 100]).expect("写入小文件");

        assert!(should_stream_file(&big), "≥ 阈值应走流式");
        assert!(!should_stream_file(&small), "小文件应走静态缓存");
        assert!(!should_stream_file(&missing), "不存在的文件不应走流式");

        // 事件稀疏度：大文件但被密集事件引用 → 仍走静态（避免掐断）
        assert!(
            should_stream_file_with_events(&big, 1),
            "大文件+稀疏事件应走流式"
        );
        assert!(
            !should_stream_file_with_events(&big, STREAMING_MAX_EVENTS + 1),
            "大文件+密集事件应走静态缓存（否则背景音被掐断）"
        );
        assert!(
            !should_stream_file_with_events(&small, 1),
            "小文件+稀疏事件也应走静态"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
