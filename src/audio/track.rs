//! 音轨管理：全部 kira `TrackHandle` 统一在一个 struct 中。

use bevy::prelude::warn;
use kira::track::{TrackBuilder, TrackHandle};

/// 全部 kira 音轨的统一管理（main → {menu, metronome, bgm, keysound} 分层）。
///
/// - **常驻轨**（menu/metronome）：与 [`super::AudioManager`] 同生命周期；
/// - **每场铺面轨**（bgm/keysound）：`begin_song` 时创建，`stop_all` 时 take + drop
///   销毁（`TrackHandle` 的 `Drop` → `mark_for_removal`，轨道上的**所有声音**
///   随之停止，不残留到下一场）。
pub(crate) struct Tracks {
    /// 主界面（选曲/标题）BGM 轨，独立于 gameplay 生命周期。
    pub(crate) menu: TrackHandle,
    /// 节拍器专用轨道（常驻合成音，独立于键音轨，避免污染 `is_playing`
    /// 对键音轨 `num_sounds` 的判定）。
    pub(crate) metronome: TrackHandle,
    /// 本场铺面 BGM 轨（流式播放）。
    pub(crate) bgm: Option<TrackHandle>,
    /// 本场铺面键音轨（静态采样，多路并发：键音 + 小 BGM 事件）。
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
        match kira.add_sub_track(TrackBuilder::default()) {
            Ok(bgm) => self.bgm = Some(bgm),
            Err(e) => warn!("[audio] 创建 BGM 轨道失败: {e}"),
        }
        match kira.add_sub_track(TrackBuilder::default()) {
            Ok(keysound) => self.keysound = Some(keysound),
            Err(e) => warn!("[audio] 创建键音轨道失败: {e}"),
        }
    }

    /// 销毁本场铺面的播放轨道（轨道上的全部声音随之停止）。
    pub(crate) fn destroy_song_tracks(&mut self) {
        self.bgm = None;
        self.keysound = None;
    }
}
