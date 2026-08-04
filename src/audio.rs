//! 全局音频管理系统：kira（cpal 后端）驱动，覆盖官方教程全部六章能力。
//!
//! 架构（对照 http://tesselo.de/kira/ 六章）：
//! - **管理器**（第 1-2 章）：`kira::AudioManager<DefaultBackend>` 单实例 Resource，
//!   后台音频线程渲染，主线程零阻塞；
//! - **播放**（第 3 章）：键音与小 BGM 采样 = `StaticSoundData`（`Arc` 共享采样、
//!   克隆廉价、可多路并发，密集事件不掐断）；大 BGM（≥ 2MB）= `StreamingSoundData`
//!   （流式后台解码，整曲不占内存）；参数（音量/播放速率/声像）支持 `Tween` 平滑过渡；
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

pub mod metronome;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Condvar, Mutex},
    thread,
};

use bevy::prelude::*;
use kira::{
    AudioManagerSettings, Decibels, DefaultBackend, Tween,
    clock::ClockHandle,
    clock::ClockSpeed,
    sound::{
        FromFileError, PlaybackState,
        static_sound::StaticSoundData,
        streaming::{StreamingSoundData, StreamingSoundHandle},
    },
    track::{TrackBuilder, TrackHandle},
};

use crate::core::settings::SettingsStore;

use self::metronome::{MetronomeData, MetronomeHandle};/// LRU 缓存上限（引用归零后仍保留的音频数）。
///
/// 每个音频在缓存生命周期内只解码一次；上限取 512 覆盖常见谱面的音频规模，
/// 减少跨游玩（LRU 淘汰后重进）时的重复解码。
const LRU_CACHE_MAX: usize = 512;

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

/// 后台解码线程池：worker 线程构造 `StaticSoundData`（kira 无后台解码 API，但
/// `from_file` 是纯 CPU 解码，可在工作线程执行；结果送回主线程缓存）。
///
/// 谱面剩余音频（首批之外）在此渐进解码，避免游玩中 `play_synced` 主线程现解卡顿。
struct DecodePool {
    tasks: Arc<Mutex<VecDeque<PathBuf>>>,
    condvar: Arc<Condvar>,
    #[allow(dead_code)] // 持有句柄保持线程存活（进程退出时随进程结束）
    _handles: Vec<thread::JoinHandle<()>>,
}

/// 后台解码线程数。
const DECODE_THREADS: usize = 4;

impl DecodePool {
    fn new(tx: mpsc::Sender<(PathBuf, Option<StaticSoundData>)>) -> Self {
        let tasks: Arc<Mutex<VecDeque<PathBuf>>> = Arc::new(Mutex::new(VecDeque::new()));
        let condvar = Arc::new(Condvar::new());
        let mut handles = Vec::new();
        for _ in 0..DECODE_THREADS {
            let tasks = tasks.clone();
            let condvar = condvar.clone();
            let tx = tx.clone();
            handles.push(thread::spawn(move || loop {
                // 取一个任务（空队列时阻塞等待）
                let path = {
                    let mut q = match tasks.lock() {
                        Ok(q) => q,
                        Err(_) => return,
                    };
                    loop {
                        if let Some(p) = q.pop_front() {
                            break p;
                        }
                        // 空队列：等待唤醒（wait 原子释放锁，唤醒后重新拿锁）
                        match condvar.wait(q) {
                            Ok(q2) => q = q2,
                            Err(_) => return,
                        }
                    }
                };
                let result = StaticSoundData::from_file(&path).ok();
                if tx.send((path, result)).is_err() {
                    break; // 接收端已销毁（应用退出）
                }
            }));
        }
        Self {
            tasks,
            condvar,
            _handles: handles,
        }
    }

    /// 提交一个解码任务。
    fn submit(&self, path: PathBuf) {
        let mut q = match self.tasks.lock() {
            Ok(q) => q,
            Err(_) => return,
        };
        q.push_back(path);
        drop(q);
        self.condvar.notify_one();
    }
}

/// 全局音频管理器（Resource）。
#[derive(Resource)]
pub struct AudioManager {
    /// kira 音频管理器（cpal 输出，音频线程渲染）。
    kira: kira::AudioManager<DefaultBackend>,
    /// 主界面（master）音轨：选曲/标题界面的 BGM，独立于 gameplay 生命周期。
    menu_track: TrackHandle,
    /// 节拍器专用轨道（常驻合成音，独立于键音轨，避免污染 `is_playing`
    /// 对键音轨 `num_sounds` 的判定）。
    #[allow(dead_code)] // 持有句柄以保持轨道存活（节拍器常驻其上）
    metronome_track: TrackHandle,
    /// 谱面时钟（BPM × 192 tick）。BGM 播放以 `clock.time()` 对齐。
    clock: ClockHandle,
    /// 节拍器（第 6 章自定义 Sound，常驻 se 轨道，tempo/开关可编程控制）。
    metronome: MetronomeHandle,
    /// 当前主界面 BGM 流（`stop_menu_bgm` 时取走停止）。
    menu_handle: Option<StreamingSoundHandle<FromFileError>>,
    /// 标记为 BGM 且**大文件**（走流式播放，不缓存）的路径。
    streaming_paths: HashSet<PathBuf>,
    /// 缓存与引用计数（与设备解耦）。
    cache: AudioCache,
    /// 后台解码池（首批之外音频渐进预加载）。
    pool: DecodePool,
    /// 解码结果队列（worker → 主线程）。
    ready_rx: Mutex<mpsc::Receiver<(PathBuf, Option<StaticSoundData>)>>,
    /// 待后台加载的剩余音频（首批提交后剩余）。
    pending_low: Vec<PathBuf>,
    /// 全局音量（0.0–1.0，线性 → main track 的 dB 音量）。
    volume: f32,
    /// 当前铺面的播放资源（轨道 + BGM 流）。`stop_all` 时 take + drop →
    /// 轨道销毁，其上**所有声音**（含静态 BGM/键音）真正停止，不残留到下一场。
    song: Option<SongAudio>,
}

/// 单场铺面的音频资源：BGM 轨 + 键音轨 + 当前 BGM 流。
///
/// drop 时两个 `TrackHandle` 触发 kira 轨道移除，轨道上的全部声音随之销毁
/// （`TrackHandle` 的 `Drop` → `mark_for_removal`）。
struct SongAudio {
    /// BGM 流式播放轨道（大文件流式解码）。
    bgm_track: TrackHandle,
    /// 键音播放轨道（静态采样，多路并发：键音 + 小 BGM 事件）。
    keysound_track: TrackHandle,
    /// 当前正在播放的 BGM 流。
    bgm_handle: Option<StreamingSoundHandle<FromFileError>>,
}

impl SongAudio {
    /// 创建本场铺面的播放轨道。
    fn new(kira: &mut kira::AudioManager<DefaultBackend>) -> Result<Self, String> {
        let bgm_track = kira
            .add_sub_track(TrackBuilder::default())
            .map_err(|e| format!("创建 BGM 轨道失败: {e}"))?;
        let keysound_track = kira
            .add_sub_track(TrackBuilder::default())
            .map_err(|e| format!("创建键音轨道失败: {e}"))?;
        Ok(Self {
            bgm_track,
            keysound_track,
            bgm_handle: None,
        })
    }
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
        let menu_track = kira
            .add_sub_track(TrackBuilder::default())
            .map_err(|_| "创建主界面音轨失败".to_string())?;
        let mut metronome_track = kira
            .add_sub_track(TrackBuilder::default())
            .map_err(|_| "创建节拍器轨道失败".to_string())?;
        // 谱面时钟：初始 120 BPM × 192 分辨率，`begin_song` 时按谱面实际 BPM 调整
        let clock = kira
            .add_clock(ClockSpeed::TicksPerMinute(120.0 * 192.0))
            .map_err(|_| "创建谱面时钟失败".to_string())?;
        // 节拍器常驻独立轨道（默认静音，`set_metronome` 开启）
        let metronome = metronome_track
            .play(MetronomeData::default())
            .map_err(|_| "启动节拍器失败".to_string())?;
        let (ready_tx, ready_rx) = mpsc::channel();
        let pool = DecodePool::new(ready_tx);
        Ok(Self {
            kira,
            menu_track,
            metronome_track,
            clock,
            metronome,
            menu_handle: None,
            streaming_paths: HashSet::new(),
            cache: AudioCache::default(),
            pool,
            ready_rx: Mutex::new(ready_rx),
            pending_low: Vec::new(),
            volume: 1.0,
            song: None,
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
    /// 加载分两批：`submit_priority` 同步解首批（前 30 秒，少量快），
    /// 其余进 `pending_low`，开玩后由 `start_low_loading` 后台渐进解码。
    pub fn acquire(&mut self, paths: &[PathBuf]) -> AudioLease {
        let lease = self.cache.acquire(paths);
        self.pending_low.extend(paths.iter().cloned());
        lease
    }

    /// 提交首批高优先级加载（前 30 秒音频），**同步解码**进缓存（量小，毫秒级）。
    ///
    /// BGM 文件跳过（流式播放，不缓存）。剩余音频留在 `pending_low`，
    /// 开玩后 `start_low_loading` 交后台线程渐进解码——避免游玩中现解卡顿。
    pub fn submit_priority(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            if !self.should_cache(&path) {
                self.pending_low.retain(|p| p != &path);
                continue; // BGM：流式播放，不缓存
            }
            self.pending_low.retain(|p| p != &path);
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

    /// 开玩后：剩余音频交后台解码池渐进加载（不阻塞主线程）。
    pub fn start_low_loading(&mut self) {
        let low = std::mem::take(&mut self.pending_low);
        let count = low.len();
        for path in low {
            if self.cache.get(&path).is_none() {
                self.pool.submit(path);
            }
        }
        if count > 0 {
            info!("[audio] {count} 个音频后台渐进解码");
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
        if self.song.is_some() {
            return;
        }
        match SongAudio::new(&mut self.kira) {
            Ok(song) => self.song = Some(song),
            Err(e) => warn!("[audio] 创建谱面播放轨道失败: {e}"),
        }
    }

    /// 谱面退出：释放租约（引用计数 -1），引用归零的按 LRU 淘汰；清空 BGM 标记。
    pub fn release(&mut self, lease: &AudioLease) {
        self.cache.release(lease);
        self.streaming_paths.clear();
    }

    /// 播放一个音频。
    ///
    /// - **大 BGM**（`register_bgm` 标记为流式）：`StreamingSoundData` + 谱面时钟对齐；
    /// - **键音与小 BGM 采样**：静态缓存播放，**多路并发不掐断**（缓存命中零拷贝，
    ///   未命中同步解码一次并缓存）。
    ///
    /// 返回是否实际发起播放（解码失败或轨道缺失时为 `false`）。
    pub fn play_synced(&mut self, path: &Path) -> bool {
        self.ensure_song_tracks();
        if !self.should_cache(path) {
            return self.play_bgm(path);
        }
        let data = match self.cache.get(path) {
            Some(data) => data.clone(),
            None => match StaticSoundData::from_file(path) {
                Ok(data) => {
                    self.cache.insert_ready(path.to_path_buf(), data.clone());
                    data
                }
                Err(e) => {
                    warn!("[audio] 同步解码失败 {}: {e}", path.display());
                    return false;
                }
            },
        };
        let Some(song) = self.song.as_mut() else {
            return false;
        };
        song.keysound_track.play(data).is_ok()
    }

    /// 播放 BGM：流式解码 + 谱面时钟对齐（音频线程精确开始）。
    ///
    /// BMS 的 BGM 事件触发语义 = 切换/重播当前 BGM：先停止旧流，避免
    /// 退出后旧 BGM 残留（kira 的流式 handle 无 Drop 停止语义，必须显式 `stop`）。
    fn play_bgm(&mut self, path: &Path) -> bool {
        let Some(song) = self.song.as_mut() else {
            return false;
        };
        if let Some(mut old) = song.bgm_handle.take() {
            old.stop(Tween::default());
        }
        match StreamingSoundData::from_file(path)
            .map(|d| d.start_time(self.clock.time()))
        {
            Ok(data) => match song.bgm_track.play(data) {
                Ok(handle) => {
                    song.bgm_handle = Some(handle);
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
            Ok(data) => match self.menu_track.play(data) {
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
        // drop SongAudio：两个 TrackHandle 触发 kira 轨道移除（`mark_for_removal`），
        // 轨道默认 `persist_until_sounds_finish=false`，下一音频回调周期即整体销毁，
        // 其上所有声音（大 BGM 流 + 静态 BGM + 键音）随之停止，无"播完才停"残留。
        self.song = None;
        self.clock.pause();
    }

    /// 是否还有音频在播放（用于完成谱面时等待背景音乐自然结束）。
    ///
    /// 检查本场谱面轨道：BGM 流 + 键音轨（键音 + 小 BGM 采样都在这条轨，
    /// `num_sounds` 不受常驻节拍器影响——节拍器在独立轨道）。
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.song.as_ref().is_some_and(|s| {
            s.bgm_handle
                .as_ref()
                .is_some_and(|h| h.state() == PlaybackState::Playing)
                || s.keysound_track.num_sounds() > 0
        })
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
                    self.refs.remove(path);
                }
            }
        }
        self.evict_if_needed();
    }

    /// 收拢一个解码完成的结果，更新 LRU 顺序并尝试淘汰。
    fn insert_ready(&mut self, path: PathBuf, data: StaticSoundData) {
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

    /// LRU 淘汰：移除最旧的**未引用**项，直到缓存不超限（保留被引用的项及其顺序）。
    fn evict_if_needed(&mut self) {
        while self.cache.len() > LRU_CACHE_MAX {
            let Some(pos) = self
                .lru
                .iter()
                .position(|p| !self.refs.contains_key(p))
            else {
                break;
            };
            let oldest = self.lru.remove(pos);
            self.cache.remove(&oldest);
            self.requested.remove(&oldest);
            self.priority_set.remove(&oldest);
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

/// 每帧收拢后台解码结果（非阻塞，渐进预加载入缓存）。
fn drain_audio_manager(mut manager: ResMut<AudioManager>) {
    manager.drain_ready();
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

        // 全部释放 → refs 清空（LRU 未超限仍保留）
        c.release(&l2);
        assert_eq!(c.refs.get(&a), None);
        assert!(c.get(&a).is_some()); // 保留在 LRU
        assert!(c.get(&b).is_some());
    }

    #[test]
    fn lru_evicts_unreferenced() {
        let mut c = AudioCache::default();
        // 制造超出 LRU 上限的未引用项
        let mut paths = Vec::new();
        for i in 0..(LRU_CACHE_MAX + 4) {
            let p = PathBuf::from(format!("/x{i}.ogg"));
            paths.push(p);
        }
        let lease = c.acquire(&paths);
        for p in &paths {
            c.insert_ready(p.clone(), static_data(8));
        }
        // 全部释放（无引用）
        c.release(&lease);
        // 超出的最旧项被淘汰
        assert!(c.cache.len() <= LRU_CACHE_MAX, "len={}", c.cache.len());
        assert!(c.get(&paths[0]).is_none(), "最旧应被淘汰");
        assert!(c.get(&paths[paths.len() - 1]).is_some(), "最新应保留");
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
    fn decode_real_audio_with_kira() {
        let home = std::env::var("HOME").unwrap_or_default();
        let ogg = PathBuf::from(home.clone()).join(".local/share/lr2oraja/songs/rainbow_ogg/1~.ogg");
        let wav = PathBuf::from(home)
            .join(".local/share/lr2oraja/songs/[hangneil+atomicsphere]tower_of_nirv/01_break_101.1.1.wav");

        let mut decoded_any = false;
        for path in [&ogg, &wav] {
            if !path.exists() {
                eprintln!("跳过 {}", path.display());
                continue;
            }
            let data = StaticSoundData::from_file(path)
                .unwrap_or_else(|e| panic!("解码失败 {}: {e}", path.display()));
            assert!(!data.frames.is_empty(), "解码结果不应为空");
            decoded_any = true;
        }
        assert!(decoded_any, "未找到任何真实音频文件，测试未实际执行");
    }

    /// BGM 路径判定：register_bgm 标记的路径走流式（不缓存），其余走静态缓存。
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




