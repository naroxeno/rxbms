//! 全局音频管理系统：低延迟混音器（cpal）+ 多线程解码池 + 缓存/引用计数/卸载。
//!
//! 架构（参考 beatoraja）：
//! - **输出**：自写混音器 [`mixer::Mixer`]（cpal 输出流 + 固定槽位池逐采样混音），
//!   短帧缓冲低延迟，播放 = 槽位复用（O(1)）；
//! - **解码**：Symphonia 解码为 [`mixer::Pcm`]（交错 f32，Arc 共享），多线程池，
//!   同 codec 复用 `AudioDecoder` 实例；
//! - **缓存**：[`AudioCache`] 引用计数 + LRU 淘汰（与设备解耦，可独立测试）；
//! - **卸载**：`AudioLease` 租约——谱面退出 `release` 引用归零 → LRU 淘汰。
//!
//! 注意：Bevy 的 `AudioPlugin` 已在 main.rs 禁用（避免双输出流冲突）。

pub mod mixer;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::File,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, mpsc,
        atomic::{AtomicUsize, Ordering},
    },
    thread::JoinHandle,

};

use bevy::prelude::*;
use symphonia::core::{
    audio::AudioSpec,
    codecs::{
        CodecParameters,
        audio::{AudioCodecId, AudioDecoder, AudioDecoderOptions},
        registry::CodecRegistry,
    },
    errors::Error,
    formats::{FormatOptions, TrackType, probe::Hint},
    io::MediaSourceStream,
    meta::MetadataOptions,
};

use crate::core::settings::SettingsStore;

use self::mixer::{Mixer, Pcm};

/// 解码 worker 线程数（上限 4）。
const DECODE_THREADS: usize = 4;
/// LRU 缓存上限（引用归零后仍保留的音频数）。
///
/// 每个音频在缓存生命周期内只解码一次；上限取 512 覆盖常见谱面的音频规模，
/// 减少跨游玩（LRU 淘汰后重进）时的重复解码。
const LRU_CACHE_MAX: usize = 512;

/// 全局音频管理器（Resource）。
#[derive(Resource)]
pub struct AudioManager {
    /// 低延迟混音器（cpal 输出流 + 槽位池）。
    mixer: Mixer,
    /// 多线程解码池。
    pool: DecodePool,
    /// 解码完成队列（worker → 主线程）。
    ready_rx: Mutex<mpsc::Receiver<(PathBuf, Option<Arc<Pcm>>)>>,
    /// 缓存与引用计数（与设备解耦）。
    cache: AudioCache,
    /// 全局音量（0.0–1.0，播放时应用到槽位）。
    volume: f32,
    /// 混音器输出采样率（同步解码兜底时统一重采样到该值）。
    sample_rate: u32,
    /// 待低优加载的剩余音频（首批提交后剩余）。
    pending_low: Vec<PathBuf>,
}

impl AudioManager {
    /// 打开系统默认输出设备并启动多线程解码池。
    ///
    /// # Errors
    ///
    /// 无可用音频设备或线程创建失败时返回错误。
    pub fn new() -> Result<Self, String> {
        let mixer = Mixer::open()?;
        let target_rate = mixer.sample_rate;
        let (ready_tx, ready_rx) = mpsc::channel::<(PathBuf, Option<Arc<Pcm>>)>();
        let pool = DecodePool::new(ready_tx, DECODE_THREADS, target_rate)?;

        Ok(Self {
            mixer,
            pool,
            ready_rx: Mutex::new(ready_rx),
            cache: AudioCache::default(),
            volume: 1.0,
            sample_rate: target_rate,
            pending_low: Vec::new(),
        })
    }

    /// 谱面进入：获取音频租约（引用计数 +1，记录全部待加载路径，**不立即提交**）。
    ///
    /// 加载分两批：`submit_priority` 提交首批（BGM + 前半段，全部 worker 快速加载），
    /// 就绪后即可开玩；`start_low_loading` 提交剩余（游玩中由 1-2 个 worker 后台加载）。
    pub fn acquire(&mut self, paths: &[PathBuf]) -> AudioLease {
        let lease = self.cache.acquire(paths);
        self.pending_low.extend(paths.iter().cloned());
        lease
    }

    /// 提交首批高优先级加载（BGM + 前半段音频），从待加载列表移除。
    pub fn submit_priority(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            self.pending_low.retain(|p| p != &path);
            self.cache.mark_priority(&path);
            self.pool.submit_high(path);
        }
    }

    /// 开始提交剩余音频（低优先级，游玩中由 1-2 个 worker 后台加载）。
    pub fn start_low_loading(&mut self) {
        let low = std::mem::take(&mut self.pending_low);
        let count = low.len();
        for path in low {
            self.pool.submit_low(path);
        }
        if count > 0 {
            info!("[audio] 剩余 {count} 个音频低优后台加载");
        }
    }

    /// 谱面退出：释放租约（引用计数 -1），引用归零的按 LRU 淘汰。
    pub fn release(&mut self, lease: &AudioLease) {
        self.cache.release(lease);
    }

    /// 收拢后台解码结果（每帧调用，非阻塞），并更新 LRU 顺序。
    pub fn drain_ready(&mut self) {
        let rx = self.ready_rx.lock().expect("ready_rx 锁失效");
        let mut items = Vec::new();
        while let Ok((path, pcm)) = rx.try_recv() {
            items.push((path, pcm));
        }
        drop(rx);
        for (path, pcm) in items {
            match pcm {
                Some(pcm) => self.cache.insert_ready(path, pcm),
                // 解码失败：从待加载集合移除，避免永久等待加载
                None => self.cache.remove_requested(&path),
            }
        }
    }

    /// 播放一个音频：已就绪则放入混音器槽位；**未就绪则同步解码一次**
    /// （小文件毫秒级，保证不丢音、不等后台队列）。beatoraja 同款懒加载语义。
    ///
    /// 返回是否实际发起播放（解码失败时为 `false`）。
    pub fn play_synced(&mut self, path: &Path) -> bool {
        if let Some(pcm) = self.cache.get(path) {
            return self.mixer.play(pcm, self.volume);
        }
        // 后台队列尚未就绪：同步解码（不占用 worker），并缓存复用
        match decode_symphonia_sync(path, self.sample_rate) {
            Ok(pcm) => {
                self.cache.insert_ready(path.to_path_buf(), pcm.clone());
                self.mixer.play(pcm, self.volume)
            }
            Err(e) => {
                warn!("[audio] 同步解码失败 {}: {e}", path.display());
                false
            }
        }
    }

    /// 停止当前所有正在播放的音频（退出谱面时调用）。
    pub fn stop_all(&mut self) {
        self.mixer.stop_all();
    }

    /// 是否还有声音在播放（用于退出时等待 BGM 自然结束）。
    #[must_use]
    pub fn is_playing(&self) -> bool {
        !self.mixer.is_idle()
    }

    /// 设置全局音量（0.0–1.0）。
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
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

/// 谱面持有的音频租约（记录占用的路径，退出时交还 `AudioManager`）。
#[derive(Debug, Default)]
pub struct AudioLease {
    paths: Vec<PathBuf>,
}

/// 音频缓存：引用计数 + LRU 淘汰 + 加载进度（纯逻辑，与设备/线程解耦）。
#[derive(Default)]
struct AudioCache {
    /// 已就绪缓存：path → PCM（Arc 共享）。
    cache: HashMap<PathBuf, Arc<Pcm>>,
    /// 引用计数：path → 持有租约数。
    refs: HashMap<PathBuf, usize>,
    /// 最近使用顺序（末尾最新，用于 LRU 淘汰）。
    lru: Vec<PathBuf>,
    /// 已请求加载的路径集合。
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
    fn insert_ready(&mut self, path: PathBuf, pcm: Arc<Pcm>) {
        self.cache.insert(path.clone(), pcm);
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

    fn get(&self, path: &Path) -> Option<Arc<Pcm>> {
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

/// 任务队列（单锁保护双队列，避免多锁等待丢失唤醒）。
struct TaskQueues {
    high: VecDeque<PathBuf>,
    low: VecDeque<PathBuf>,
}

impl TaskQueues {
    fn new() -> Self {
        Self {
            high: VecDeque::new(),
            low: VecDeque::new(),
        }
    }
}

/// 多线程解码池：优先级双队列 + N 个 worker。
///
/// - **High 队列**（首批/BGM）：所有 worker 可处理，优先取出；
/// - **Low 队列**（游玩中后续加载）：最多 [`LOW_WORKER_LIMIT`] 个 worker 同时处理，
///   避免解码占用过多 CPU 影响游玩帧率。
///
/// 双队列共用一把锁 + 一个条件变量：任何提交都会唤醒等待中的 worker，
/// 唤醒后重新检查两个队列（避免只等 Low 却错过 High 的唤醒丢失）。
struct DecodePool {
    tasks: Arc<Mutex<TaskQueues>>,
    condvar: Arc<Condvar>,
    /// 正在处理 Low 任务的 worker 数（仅 worker 线程经 Arc clone 访问）。
    #[allow(dead_code)]
    low_active: Arc<AtomicUsize>,
    _handles: Vec<JoinHandle<()>>,
}

/// Low（后续加载）任务最多同时处理的 worker 数。
const LOW_WORKER_LIMIT: usize = 2;

impl DecodePool {
    /// 启动 `threads` 个解码 worker。
    ///
    /// # Errors
    ///
    /// 线程创建失败时返回错误。
    fn new(
        ready_tx: mpsc::Sender<(PathBuf, Option<Arc<Pcm>>)>,
        threads: usize,
        target_rate: u32,
    ) -> Result<Self, String> {
        let tasks = Arc::new(Mutex::new(TaskQueues::new()));
        let condvar = Arc::new(Condvar::new());
        let low_active = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::with_capacity(threads);
        for _ in 0..threads {
            let tasks = Arc::clone(&tasks);
            let condvar = Arc::clone(&condvar);
            let low_active = Arc::clone(&low_active);
            let ready_tx = ready_tx.clone();
            let handle = std::thread::Builder::new()
                .name("rxbms-audio-decode".into())
                .spawn(move || {
                    // 每个 worker 维护一个可复用的 Symphonia 音频解码器（同 codec 复用，
                    // 避免每个文件都创建新的解码器实例）。
                    let registry = symphonia::default::get_codecs();
                    let mut cached_decoder: Option<(AudioCodecId, Box<dyn AudioDecoder>)> = None;
                    loop {
                        // 取任务：优先 High；Low 受并发上限约束；都不可取才等待
                        let task: Option<(PathBuf, bool)> = {
                            let mut q = tasks.lock().expect("解码任务队列锁失效");
                            loop {
                                if let Some(t) = q.high.pop_front() {
                                    break Some((t, false));
                                }
                                let active = low_active.load(Ordering::Relaxed);
                                if active < LOW_WORKER_LIMIT
                                    && let Some(t) = q.low.pop_front()
                                {
                                    low_active.fetch_add(1, Ordering::Relaxed);
                                    break Some((t, true));
                                }
                                q = condvar.wait(q).expect("解码任务队列等待失效");
                            }
                        };
                        if let Some((path, is_low)) = task {
                            match decode_symphonia(&path, registry, &mut cached_decoder, target_rate)
                            {
                                Ok(pcm) => {
                                    let _ = ready_tx.send((path, Some(pcm)));
                                }
                                Err(e) => {
                                    warn!("[audio] 解码失败 {}: {e}", path.display());
                                    // 失败也回传（None），让主线程跳过该文件不再等待
                                    let _ = ready_tx.send((path, None));
                                }
                            }
                            if is_low {
                                low_active.fetch_sub(1, Ordering::Relaxed);
                            }
                            condvar.notify_one(); // 唤醒等待的 worker（Low 槽位或新任务）
                        }
                    }
                })
                .map_err(|e| format!("启动解码线程失败: {e}"))?;
            handles.push(handle);
        }
        Ok(Self {
            tasks,
            condvar,
            low_active,
            _handles: handles,
        })
    }

    /// 提交高优先级任务（首批，唤醒一个 worker）。
    fn submit_high(&self, path: PathBuf) {
        self.tasks
            .lock()
            .expect("解码任务队列锁失效")
            .high
            .push_back(path);
        self.condvar.notify_one();
    }

    /// 提交低优先级任务（游玩中后续加载，唤醒一个 worker）。
    fn submit_low(&self, path: PathBuf) {
        self.tasks
            .lock()
            .expect("解码任务队列锁失效")
            .low
            .push_back(path);
        self.condvar.notify_one();
    }
}

/// 单文件同步解码（播放兜底用）：不复用解码器，每次新建。
fn decode_symphonia_sync(path: &Path, target_rate: u32) -> Result<Arc<Pcm>, String> {
    let registry = symphonia::default::get_codecs();
    decode_symphonia(path, registry, &mut None, target_rate)
}

/// 用 Symphonia 解码音频文件为 [`Pcm`]（交错 f32）。worker 线程执行。
///
/// `cached_decoder` 实现**解码器复用**：同一 worker 处理同 codec（如 ogg/vorbis）的
/// 连续文件时，复用已实例化的 `AudioDecoder`（`reset()` 后重新用于新流）。
/// 输出统一重采样到 `target_rate`（混音器输出采样率），避免混音端二次转换。
fn decode_symphonia(
    path: &Path,
    registry: &CodecRegistry,
    cached_decoder: &mut Option<(AudioCodecId, Box<dyn AudioDecoder>)>,
    target_rate: u32,
) -> Result<Arc<Pcm>, String> {
    let file = File::open(path).map_err(|e| format!("打开失败: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| format!("格式探测失败: {e}"))?;
    let track = format
        .first_track(TrackType::Audio)
        .ok_or_else(|| "无音频轨道".to_string())?;
    // 拷贝所需数据，避免借用 format 阻碍后续 next_packet（&mut）
    let track_id = track.id;
    let codec_params = track
        .codec_params
        .clone()
        .ok_or_else(|| "无音频编解码参数".to_string())?;
    let CodecParameters::Audio(params) = codec_params else {
        return Err("无音频编解码参数".into());
    };
    let codec_id = params.codec;

    // 复用或新建解码器（同 codec 复用并 reset，否则重建）
    let mut decoder: Box<dyn AudioDecoder> = match cached_decoder.take() {
        Some((id, mut d)) if id == codec_id => {
            d.reset();
            d
        }
        _ => {
            let registered = registry
                .get_audio_decoder(codec_id)
                .ok_or_else(|| "不支持的音频编码".to_string())?;
            (registered.factory)(&params, &AudioDecoderOptions::default())
                .map_err(|e| format!("创建解码器失败: {e}"))?
        }
    };

    // 解码循环：逐 packet 解码并交错收集为 f32
    let mut all: Vec<f32> = Vec::new();
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    while let Ok(Some(packet)) = format.next_packet() {
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec: &AudioSpec = decoded.spec();
                channels = spec.channels().count() as u16;
                sample_rate = spec.rate();
                let frames = decoded.frames();
                let mut out = vec![0.0f32; frames * channels as usize];
                decoded.copy_to_slice_interleaved::<f32, _>(&mut out);
                all.extend_from_slice(&out);
            }
            Err(Error::DecodeError(_)) => continue, // 跳过损坏包
            Err(_) => break,                        // ResetRequired 等 → 结束本次
        }
    }

    // 回存解码器供后续文件复用（失败路径不缓存）
    *cached_decoder = Some((codec_id, decoder));

    if channels == 0 || sample_rate == 0 || all.is_empty() {
        return Err("无有效音频数据".into());
    }

    // 统一到混音器输出采样率（线性重采样）
    if sample_rate != target_rate {
        let ch = channels as usize;
        all = resample_linear(&all, ch, sample_rate, target_rate);
        sample_rate = target_rate;
    }

    Ok(Arc::new(Pcm::new(all, channels, sample_rate)))
}

/// 简单线性插值重采样（交错平面，`from` → `to`）。
fn resample_linear(samples: &[f32], channels: usize, from: u32, to: u32) -> Vec<f32> {
    if from == to || channels == 0 || samples.len() < channels {
        return samples.to_vec();
    }
    let src_frames = samples.len() / channels;
    let dst_frames = ((src_frames as f64 * f64::from(to) / f64::from(from)) as usize).max(1);
    let ratio = f64::from(to) / f64::from(from);
    let mut out = vec![0.0f32; dst_frames * channels];
    for ch in 0..channels {
        for i in 0..dst_frames {
            let pos = i as f64 / ratio;
            let i0 = (pos.floor() as usize).min(src_frames - 1);
            let i1 = (i0 + 1).min(src_frames - 1);
            let frac = (pos - pos.floor()) as f32;
            let a = samples[i0 * channels + ch];
            let b = samples[i1 * channels + ch];
            out[i * channels + ch] = a + (b - a) * frac;
        }
    }
    out
}

/// 音频管理插件：初始化管理器 + 每帧收拢解码结果。
pub struct AudioManagerPlugin;

impl Plugin for AudioManagerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init_audio_manager)
            .add_systems(Update, drain_audio_manager);
    }
}

fn init_audio_manager(mut commands: Commands) {
    match AudioManager::new() {
        Ok(manager) => {
            info!(
                "[audio] 音频系统就绪（{} 线程解码，输出 {}Hz）",
                DECODE_THREADS,
                manager.mixer.sample_rate
            );
            commands.insert_resource(manager);
        }
        Err(e) => error!("[audio] 初始化失败: {e}"),
    }
}

/// 每帧收拢后台解码结果（全局，非阻塞），并同步音量设置。
fn drain_audio_manager(mut manager: ResMut<AudioManager>, store: Res<SettingsStore>) {
    manager.drain_ready();
    let volume = store.get_float("volume", 1.0) as f32;
    if (manager.volume - volume).abs() > 0.001 {
        manager.set_volume(volume);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use self::mixer::Pcm as TestPcm;

    fn pcm(samples: Vec<f32>, channels: u16) -> Arc<Pcm> {
        Arc::new(Pcm::new(samples, channels, 44_100))
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
        c.insert_ready(a.clone(), pcm(vec![0.0; 8], 1));
        c.insert_ready(b.clone(), pcm(vec![0.0; 8], 1));
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
            c.insert_ready(p.clone(), pcm(vec![0.0; 8], 1));
        }
        // 全部释放（无引用）
        c.release(&lease);
        // 超出的最旧项被淘汰
        assert!(c.cache.len() <= LRU_CACHE_MAX, "len={}", c.cache.len());
        assert!(c.get(&paths[0]).is_none(), "最旧应被淘汰");
        assert!(c.get(&paths[paths.len() - 1]).is_some(), "最新应保留");
    }

    /// 线性重采样（采样率统一逻辑）。
    #[test]
    fn sample_rate_normalization() {
        // 22050 → 44100（2x 上采样，帧数翻倍）
        let mono: Vec<f32> = (0..10).map(|i| i as f32).collect();
        let up = resample_linear(&mono, 1, 22_050, 44_100);
        assert_eq!(up.len(), 20);
        assert!((up[0] - 0.0).abs() < 1e-4);
        assert!((up[19] - 9.0).abs() < 1e-4);

        // 同采样率原样返回
        assert_eq!(resample_linear(&mono, 1, 44_100, 44_100), mono);

        // 立体声交错
        let stereo: Vec<f32> = (0..8).map(|i| i as f32).collect(); // 4 帧
        let out = resample_linear(&stereo, 2, 44_100, 48_000);
        assert!(out.len() % 2 == 0);
        assert_eq!(out.len() / 2, 4, "4 帧 × 48/44.1 ≈ 4.35 → floor 4");
    }

    /// 真实音频解码 + 解码器复用（文件不存在时跳过）。
    #[test]
    fn decode_real_audio_with_reuse() {
        let home = std::env::var("HOME").unwrap_or_default();
        let ogg = PathBuf::from(home.clone()).join(".local/share/lr2oraja/songs/rainbow_ogg/1~.ogg");
        let wav = PathBuf::from(home)
            .join(".local/share/lr2oraja/songs/[hangneil+atomicsphere]tower_of_nirv/01_break_101.1.1.wav");
        let registry = symphonia::default::get_codecs();
        let mut cached: Option<(AudioCodecId, Box<dyn AudioDecoder>)> = None;

        let mut decoded_any = false;
        for path in [&ogg, &wav] {
            if !path.exists() {
                eprintln!("跳过 {}", path.display());
                continue;
            }
            let pcm = decode_symphonia(path, registry, &mut cached, 44_100)
                .unwrap_or_else(|e| panic!("解码失败 {}: {e}", path.display()));
            assert!(!pcm.samples.is_empty(), "解码结果不应为空");
            assert_eq!(pcm.sample_rate, 44_100, "采样率应统一到目标");
            decoded_any = true;
        }
        // 连续两个同格式文件时 cached 应被复用（不报错即通过）
        if ogg.exists() {
            let _ = decode_symphonia(&ogg, registry, &mut cached, 44_100)
                .expect("复用解码器再解码同格式文件");
        }
        assert!(decoded_any, "未找到任何真实音频文件，测试未实际执行");
        let _ = TestPcm::new(vec![], 0, 0); // 保持类型引用
    }
}
