//! BGA（背景动画）系统：BMS 背景图片/视频的显示与切换（参考 beatoraja）。
//!
//! - **数据**：[`BgaData`]——Base 层事件流（秒 → BmpId）+ 图片/视频文件映射；
//! - **静态图**：AssetServer 懒加载（`fs://`），事件触发时切换 Sprite 贴图；
//! - **视频**：ffmpeg 解码（`ffmpeg-next`，动态链接系统 ffmpeg；跨平台可用
//!   `static-ffmpeg` feature 静态编译），按谱面时间顺序解码帧，写入 Bevy `Image`。
//!
//! 播放器使用 `NonSend` Resource（ffmpeg 对象仅主线程访问，避免 Send/Sync 约束）。
//! 简化（TODO）：仅 Base 层；视频从触发点顺序播放（未处理 `#STARTxx`/`#ENDxx`
//! 时间窗口、Overlay/Poor 层、透明度/ARGB 动画）。

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc,
    },
};

use bevy::{
    asset::{AssetServer, RenderAssetUsages},
    image::Image,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use ffmpeg_next as ffmpeg;

/// BGA 事件（Base 层）：谱面时间 → BmpId。
#[derive(Debug, Clone, Copy)]
pub struct BgaEvent {
    /// 触发时间（秒）。
    pub time_sec: f64,
    /// BGA/BMP 资源 id。
    pub bmp_id: usize,
}

/// BGA 数据（谱面加载时提取）。
#[derive(Debug, Default, Clone)]
pub struct BgaData {
    /// Base 层事件流（按时间排序）。
    pub events: Vec<BgaEvent>,
    /// 静态图：BmpId → 磁盘路径。
    pub images: HashMap<usize, PathBuf>,
    /// 视频：BmpId → 磁盘路径。
    pub videos: HashMap<usize, PathBuf>,
}

impl BgaData {
    /// 是否有任何 BGA 资源。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty() && self.images.is_empty() && self.videos.is_empty()
    }
}

/// BGA 播放器（Resource）：事件驱动切换 + 视频帧解码。
///
/// BGA 图像交给**皮肤系统**渲染（beatoraja `skin.bga` destination）：
/// 本播放器只负责维护"当前 BGA 图像"（静态图 handle 或视频帧 handle），
/// 皮肤渲染时经 [`BgaPlayer::current_image`] 取用。
///
/// ffmpeg 对象仅主线程访问；`unsafe impl Send` 声明其可安全跨线程移动
/// （libav* 读取操作线程安全，且本对象从不跨线程并发使用）。
#[derive(Resource)]
pub struct BgaPlayer {
    data: BgaData,
    /// 下一个待触发事件索引。
    next_idx: usize,
    /// 当前显示的 BmpId（None = 无 BGA）。
    current: Option<usize>,
    /// 静态图句柄（懒加载）：BmpId → Handle<Image>。
    images: HashMap<usize, Handle<Image>>,
    /// 视频帧缓冲区句柄（预创建，每帧更新）。
    frame_image: Handle<Image>,
    /// 当前视频流（后台线程解码）。
    video: Option<VideoStream>,
    /// 当前视频帧尺寸（descriptor 只在尺寸变化时更新，避免纹理重建卡顿）。
    video_size: Option<(u32, u32)>,
    /// 当前 BGA 图像（供皮肤渲染）：静态图 handle 或视频帧 handle。
    current_image: Option<Handle<Image>>,
}

impl BgaPlayer {
    /// 创建播放器（仅准备视频帧缓冲，不生成实体；渲染由皮肤 destination 完成）。
    pub fn new(data: BgaData, images: &mut Assets<Image>) -> Self {
        let frame_image = images.add(Image::new(
            Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            vec![0u8; 16], // 2×2 BGRA 占位（黑；GPU shader 重排为 RGBA）
            TextureFormat::Bgra8UnormSrgb,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        ));

        Self {
            data,
            next_idx: 0,
            current: None,
            images: HashMap::new(),
            frame_image,
            video: None,
            video_size: None,
            current_image: None,
        }
    }

    /// 当前 BGA 图像句柄（皮肤渲染用；None = 无 BGA 显示）。
    #[must_use]
    pub fn current_image(&self) -> Option<Handle<Image>> {
        self.current_image.clone()
    }

    /// 每帧更新：触发事件切换 + 视频帧推进。
    pub fn update(
        &mut self,
        now_sec: f64,
        asset_server: &AssetServer,
        images: &mut Assets<Image>,
    ) {
        // 1. 触发到达的事件
        while self.next_idx < self.data.events.len()
            && self.data.events[self.next_idx].time_sec <= now_sec
        {
            let ev = self.data.events[self.next_idx];
            self.next_idx += 1;
            if ev.bmp_id != self.current.unwrap_or(usize::MAX) {
                // 视频从**触发时刻**从头播放（beatoraja restart 校准）
                self.switch_to(ev.bmp_id, now_sec, asset_server);
            }
        }

        // 2. 视频帧推进（写入帧缓冲；皮肤渲染经 current_image 取用）
        if let Some(stream) = &mut self.video {
            stream.set_target(now_sec);
            stream.drain();
            if let Some(f) = stream.frame_at(now_sec)
                && let Some(mut img) = images.get_mut(&self.frame_image)
            {
                img.data = Some(f.rgba);
                // descriptor 仅在尺寸变化时更新（同尺寸每帧改会触发纹理重建 → 卡顿）
                if self.video_size != Some((f.w, f.h)) {
                    img.texture_descriptor.size = Extent3d {
                        width: f.w,
                        height: f.h,
                        depth_or_array_layers: 1,
                    };
                    img.texture_descriptor.format = TextureFormat::Bgra8UnormSrgb;
                    self.video_size = Some((f.w, f.h));
                }
            }
            // 解码未到达或已播完 → 保持上一帧
        }
    }

    /// 切换到指定 BmpId：图片 / 视频 / 无资源 → 更新当前图像。
    ///
    /// `trigger_sec`：视频触发时刻（谱面时间），视频从此刻从头播放。
    fn switch_to(&mut self, bmp_id: usize, trigger_sec: f64, asset_server: &AssetServer) {
        self.current = Some(bmp_id);
        // 停止旧视频
        self.video = None;
        self.video_size = None;

        if let Some(path) = self.data.images.get(&bmp_id) {
            // 静态图：懒加载，设为当前图像
            let handle = self
                .images
                .entry(bmp_id)
                .or_insert_with(|| asset_server.load(format!("fs://{}", path.display())))
                .clone();
            self.current_image = Some(handle);
        } else if let Some(path) = self.data.videos.get(&bmp_id) {
            // 视频：启动后台解码线程，当前图像 = 帧缓冲
            match VideoStream::start(path, trigger_sec) {
                Ok(video) => {
                    self.video = Some(video);
                    self.current_image = Some(self.frame_image.clone());
                }
                Err(e) => {
                    warn!("[bga] 视频打开失败 {}: {e}", path.display());
                    self.current_image = None;
                }
            }
        } else {
            // 无资源（该 id 未定义文件）→ 无 BGA
            self.current_image = None;
        }
    }
}

/// 视频解码线程 → 主线程的一帧（RGBA 已转好）。
struct VideoFrame {
    /// 帧时间戳（视频内秒）。
    sec: f64,
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

/// 视频流：**专用后台线程**持续顺序解码，主线程按谱面时间取帧。
///
/// 时间模型（beatoraja）：视频从**触发时刻**从头播放，
/// `offset = 首帧时间戳 − 触发时刻`，目标帧 = 谱面时间 + offset。
/// 解码按目标时间节流（超前 sleep，不积压），主线程零解码。
struct VideoStream {
    rx: mpsc::Receiver<Option<VideoFrame>>,
    stop: Arc<AtomicBool>,
    /// 目标帧时间偏移（秒）：帧选择用 `sec <= 谱面时间 + offset`。
    offset: f64,
    /// 当前谱面时间（主线程写入，解码线程节流读取；纳秒）。
    target_ns: Arc<AtomicU64>,
    /// 已解码但未消费的帧（按 sec 升序，限长）。
    frames: VecDeque<VideoFrame>,
    /// 是否已收到 EOF。
    eof: bool,
}

/// 帧缓冲上限（约半秒超前量；解码再快也只保留最近 16 帧）。
const VIDEO_BUFFER_MAX: usize = 16;

impl VideoStream {
    /// 打开视频并启动后台解码线程。
    ///
    /// `trigger_sec`：该视频首次触发的谱面时间（beatoraja `restart()` 校准基准）。
    fn start(path: &Path, trigger_sec: f64) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let target_ns = Arc::new(AtomicU64::new((trigger_sec * 1e9) as u64));
        let target_ns2 = target_ns.clone();
        // Box 包裹：避免 Rust 2021 闭包字段级捕获（disjoint capture）把非 Send 的
        // scaler 字段单独捕获进线程（整个 BgaVideo 有 unsafe impl Send，字段没有）。
        let mut video = Box::new(BgaVideo::open(path)?);
        // beatoraja：`offset = grabber.getTimestamp() − time*1000`（首帧时间戳 − 触发时刻）
        let offset = video.start_ts - trigger_sec;
        std::thread::Builder::new()
            .name("bga-decode".into())
            .spawn(move || {
                for (stream, packet) in video.ictx.packets() {
                    if stop2.load(Ordering::Relaxed) {
                        break;
                    }
                    if stream.index() != video.stream_index {
                        continue;
                    }
                    if video.decoder.send_packet(&packet).is_err() {
                        continue;
                    }
                    let mut frame = ffmpeg::frame::Video::empty();
                    while video.decoder.receive_frame(&mut frame).is_ok() {
                        let sec = frame
                            .timestamp()
                            .map_or(video.decoded_sec, |ts| ts as f64 * video.time_base);
                        video.decoded_sec = sec;
                        let mut rgba = ffmpeg::frame::Video::empty();
                        if video.scaler.run(&frame, &mut rgba).is_err() {
                            continue;
                        }
                        let f = VideoFrame {
                            sec,
                            w: rgba.width(),
                            h: rgba.height(),
                            rgba: rgba.data(0).to_vec(),
                        };
                        if tx.send(Some(f)).is_err() {
                            return; // 接收端已放弃
                        }
                        // 节流：解码超前目标 0.5s 以上 → 睡一会（跟随谱面时间，不积压）
                        let target = target_ns2.load(Ordering::Relaxed) as f64 / 1e9;
                        if sec > target + 0.5 {
                            std::thread::sleep(std::time::Duration::from_millis(4));
                        }
                    }
                }
                let _ = tx.send(None); // EOF
            })
            .map_err(|e| format!("视频解码线程创建失败: {e}"))?;
        Ok(Self {
            rx,
            stop,
            offset,
            target_ns,
            frames: VecDeque::new(),
            eof: false,
        })
    }

    /// 更新当前谱面时间（解码线程节流基准）。
    fn set_target(&mut self, now_sec: f64) {
        self.target_ns
            .store((now_sec * 1e9) as u64, Ordering::Relaxed);
    }

    /// 拉取解码线程已产出的帧到缓冲（非阻塞），限长。
    fn drain(&mut self) {
        while !self.eof && self.frames.len() < VIDEO_BUFFER_MAX {
            match self.rx.try_recv() {
                Ok(Some(f)) => self.frames.push_back(f),
                Ok(None) => {
                    self.eof = true;
                    break;
                }
                Err(_) => break, // 暂时无帧
            }
        }
    }

    /// 取 `sec <= target` 的最近一帧（显示它并移除更早的帧）。
    /// 目标时间含触发偏移（beatoraja：`microtime = time*1000 + offset`）。
    /// 解码尚未到达时返回 `None`（保持上一帧显示）。
    fn frame_at(&mut self, target: f64) -> Option<VideoFrame> {
        let t = target + self.offset;
        let mut idx = None;
        for (i, f) in self.frames.iter().enumerate() {
            if f.sec <= t {
                idx = Some(i);
            } else {
                break;
            }
        }
        let i = idx?;
        self.frames.drain(..i); // 丢弃已显示过的更早帧
        self.frames.pop_front()
    }
}

impl Drop for VideoStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// ffmpeg 视频解码器（**仅后台解码线程使用**；主线程不接触）。
struct BgaVideo {
    ictx: ffmpeg::format::context::Input,
    decoder: ffmpeg::codec::decoder::Video,
    scaler: ffmpeg::software::scaling::Context,
    stream_index: usize,
    /// 帧时间戳单位（秒/时间戳，取**流的** time_base）。
    time_base: f64,
    /// 视频首帧时间戳（秒；无 → 0）。
    start_ts: f64,
    /// 已解码游标（视频内秒）。
    decoded_sec: f64,
}

// SAFETY: BgaVideo 仅被**单个**后台解码线程独占使用（从不跨线程并发访问），
// 与 BgaPlayer 同理（libav* 读取操作线程安全）。
unsafe impl Send for BgaVideo {}

impl BgaVideo {
    /// 打开视频文件并准备解码。
    ///
    /// # Errors
    ///
    /// ffmpeg 初始化、打开文件或获取视频流失败时返回错误。
    fn open(path: &Path) -> Result<Self, String> {
        ffmpeg::init().map_err(|e| format!("ffmpeg 初始化失败: {e}"))?;
        let ictx =
            ffmpeg::format::input(path).map_err(|e| format!("打开视频失败: {e}"))?;
        let input = ictx
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or_else(|| "无视频流".to_string())?;
        let stream_index = input.index();
        // 帧 timestamp 以**流** time_base 为单位（容器 time_base 可能不同，
        // 用容器换算会导致帧时间错误 → 视频速度与音乐不同步）
        let tb = input.time_base();
        let time_base = if tb.0 > 0 {
            tb.0 as f64 / tb.1 as f64
        } else {
            1.0 / 30.0
        };
        // 视频首帧时间戳（秒；AV_NOPTS_VALUE/负值 → 0）
        let start_ts = input.start_time();
        let start_ts = if start_ts > 0 {
            start_ts as f64 * time_base
        } else {
            0.0
        };
        let ctx = ffmpeg::codec::context::Context::from_parameters(input.parameters())
            .map_err(|e| format!("解码器上下文失败: {e}"))?;
        let decoder = ctx
            .decoder()
            .video()
            .map_err(|e| format!("视频解码器失败: {e}"))?;
        let width = decoder.width();
        let height = decoder.height();
        let format = decoder.format();
        let scaler = ffmpeg::software::scaling::Context::get(
            format,
            width,
            height,
            ffmpeg::format::Pixel::BGRA,
            width,
            height,
            ffmpeg::software::scaling::Flags::BILINEAR,
        )
        .map_err(|e| format!("缩放器创建失败: {e}"))?;
        // 注：ffmpeg-next 8.x 的 `Video(pub Opened)` 已处于打开状态，无需再 open()

        Ok(Self {
            ictx,
            decoder,
            scaler,
            stream_index,
            time_base,
            start_ts,
            decoded_sec: 0.0,
        })
    }
}

// SAFETY: ffmpeg 对象（Input/decoder/scaler）仅在本播放器的游戏主线程中访问，
// 从不跨线程并发使用（不 Send 也不 Sync 的 C 指针/Rc 状态仅单线程接触）。
// Bevy Resource 需 Send + Sync。
unsafe impl Send for BgaPlayer {}
unsafe impl Sync for BgaPlayer {}

#[allow(dead_code)]
fn _assert_send<T: Send>() {}
// 编译期强制检查：BgaVideo 必须 Send（否则 const 初始化失败）
fn assert_send<T: Send>() {}
#[allow(dead_code)]
const _: fn() = assert_send::<BgaVideo>;
