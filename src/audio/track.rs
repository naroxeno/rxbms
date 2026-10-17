//! 音轨管理：全部 kira `TrackHandle` 统一在一个 struct 中。

use bevy::prelude::warn;
use kira::track::{TrackBuilder, TrackHandle};

/// 长 BGM 音轨容量：同一时刻正在播放的长 BGM 数。
///
/// 叠加语义下不再"停止旧曲"，同路径去重只挡同一路径；不同路径的循环段落
/// 可以叠播，容量取 32 留余量（超出由 kira 拒绝并打 warn）。
pub(crate) const BGM_SOUND_CAPACITY: usize = 32;

/// 一次性 BGM 采样音轨容量（kira 硬上限，超出时 `play` 返回
/// `SoundLimitReached` → 丢音）。
///
/// 密集谱面（鼓组 BGM 每 16/32 分音符一个事件）同时发声数可达百余个；
/// kira 子轨默认只给 128，会把后续采样直接拒绝播放。这里放宽到 512，
/// 配合 `AudioManager::bgm_samples` 的软上限（256，超出停最旧）防止
/// 无节制堆叠与内存膨胀。
pub(crate) const BGM_SAMPLE_SOUND_CAPACITY: usize = 512;

/// 键音音轨容量：键音并发由 `KEYSOUND_MAX_TOTAL`（32）软限制，128 留余量。
pub(crate) const KEYSOUND_SOUND_CAPACITY: usize = 128;

/// 全部 kira `TrackHandle` 的统一管理（main → {menu, metronome, bgm, bgm_sample, keysound} 分层）。
///
/// - **常驻轨**（menu/metronome）：与 [`super::AudioManager`] 同生命周期；
/// - **每场铺面轨**（bgm/bgm_sample/keysound）：`begin_song` 时创建，`stop_all` 时 take + drop
///   销毁（`TrackHandle` 的 `Drop` → `mark_for_removal`，轨道上的**所有声音**
///   随之停止，不残留到下一场）。
pub(crate) struct Tracks {
    /// 主界面（选曲/标题）BGM 轨，独立于 gameplay 生命周期。
    pub(crate) menu: TrackHandle,
    /// 节拍器专用轨道（常驻合成音，独立于键音轨，避免污染 `is_playing`
    /// 对键音轨 `num_sounds` 的判定）。
    pub(crate) metronome: TrackHandle,
    /// 本场铺面 BGM 轨（长 BGM：流式或长静态采样，切换/去重语义）。
    pub(crate) bgm: Option<TrackHandle>,
    /// 本场铺面一次性 BGM 采样轨（短采样，每个事件都发声，容量放宽）。
    pub(crate) bgm_sample: Option<TrackHandle>,
    /// 本场铺面键音轨（静态采样，多路并发）。
    pub(crate) keysound: Option<TrackHandle>,
}

impl Tracks {
    /// 创建常驻轨道（menu + metronome）。
    pub(crate) fn new(kira: &mut kira::AudioManager<kira::DefaultBackend>) -> Result<Self, String> {
        let menu = kira
            .add_sub_track(TrackBuilder::default())
            .map_err(|_| "创建主界面音轨失败".to_string())?;
        let metronome = kira
            .add_sub_track(TrackBuilder::default())
            .map_err(|_| "创建节拍器轨道失败".to_string())?;
        Ok(Self {
            menu,
            metronome,
            bgm: None,
            bgm_sample: None,
            keysound: None,
        })
    }

    /// 确保本场铺面的播放轨道已创建（上一场 `destroy_song_tracks` 销毁后重建）。
    pub(crate) fn ensure_song_tracks(
        &mut self,
        kira: &mut kira::AudioManager<kira::DefaultBackend>,
    ) {
        if self.bgm.is_some() {
            return;
        }
        self.bgm = Self::add_track(kira, "BGM", BGM_SOUND_CAPACITY);
        self.bgm_sample = Self::add_track(kira, "BGM 采样", BGM_SAMPLE_SOUND_CAPACITY);
        self.keysound = Self::add_track(kira, "键音", KEYSOUND_SOUND_CAPACITY);
    }

    /// 创建一条子轨并指定同时发声上限；失败时告警并返回 `None`。
    fn add_track(
        kira: &mut kira::AudioManager<kira::DefaultBackend>,
        name: &str,
        sound_capacity: usize,
    ) -> Option<TrackHandle> {
        match kira.add_sub_track(TrackBuilder::new().sound_capacity(sound_capacity)) {
            Ok(track) => Some(track),
            Err(e) => {
                warn!("[audio] 创建{name}轨道失败: {e}");
                None
            }
        }
    }

    /// 销毁本场铺面的播放轨道（轨道上的全部声音随之停止）。
    pub(crate) fn destroy_song_tracks(&mut self) {
        self.bgm = None;
        self.bgm_sample = None;
        self.keysound = None;
    }
}
