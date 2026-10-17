//! 统一资源管理：一场谱面占用的资源（音频/BGA/皮肤/视觉）集中登记与统一释放。
//!
//! 动机：音频解码缓存与 BGA handle 曾因清理逻辑散落各处（`teardown_gameplay`
//! 与 `hide_skin_slots` 两个 `OnExit` 系统、执行顺序未定义）而泄漏。集中到
//! **单一登记**（[`ResourceManager`]）+ **单一释放入口**（[`cleanup_gameplay_resources`]）
//! 后，新增资源类型只需在此登记并在释放入口中处理，不再散落遗漏。

use bevy::prelude::*;

use crate::{
    audio::{AudioLease, AudioManager},
    skin::runtime::SkinRuntime,
};

use super::{
    bga::BgaPlayer,
    data::GameplayData,
    judge::{GaugeState, JudgeState},
    lane::LaneStates,
    render::GameplayVisual,
    GameplayLoadFailed, GameplayRender, GameplaySession,
};

/// 统一资源管理器：登记一场谱面的资源占用，退出时按序统一释放。
#[derive(Resource, Default)]
pub struct ResourceManager {
    /// 本场谱面音频租约（退出时交还 `AudioManager`，解码缓存立即回收）。
    audio_lease: Option<AudioLease>,
    /// 资源统计（退出时日志输出，便于核对释放）。
    stats: ResourceStats,
}

/// 资源统计（`setup_gameplay` 登记，退出时打印）。
#[derive(Debug, Default, Clone, Copy)]
pub struct ResourceStats {
    /// 本场谱面引用的音频路径数。
    pub audio_paths: usize,
    /// BGA 静态图数。
    pub bga_images: usize,
    /// BGA 视频数。
    pub bga_videos: usize,
}

impl ResourceManager {
    /// 登记音频租约（`setup_gameplay` 调用）。
    ///
    /// 正常流程下上一场租约已由 [`cleanup_gameplay_resources`] 取走释放；
    /// 此处防御性检查——异常路径（如音频初始化失败导致 cleanup 未执行
    /// 释放）残留的旧租约不得静默覆盖泄漏。
    pub fn acquire_audio(&mut self, lease: AudioLease) {
        debug_assert!(
            self.audio_lease.is_none(),
            "上一场音频租约未释放（cleanup 未执行？）"
        );
        self.audio_lease = Some(lease);
    }

    /// 登记资源统计。
    pub fn record_stats(&mut self, stats: ResourceStats) {
        self.stats = stats;
    }
}

/// 统一释放全部游玩资源（`OnExit(Gameplay)` 单一入口，替代原分散的
/// `teardown_gameplay` + `hide_skin_slots`）。
///
/// 释放顺序（依赖安全）：
/// 1. **停止音频播放**（`stop_all` 销毁本场铺面轨道，先停声再释放数据）；
/// 2. **释放音频租约** → 引用归零的解码缓存立即回收；
/// 3. **销毁游玩视觉实体**（音符/特效，释放其 handle）；
/// 4. **销毁皮肤槽实体**（释放 BGA 帧缓冲/静态图/纹理 handle；下次进入
///    Gameplay 由 `load_lua_skin` 重建，其清理逻辑对已清空列表无害）；
/// 5. **移除游玩资源**（`BgaPlayer` drop → 停止视频解码线程并释放 ffmpeg
///    对象；`GameplaySession` 等 drop → 释放谱面数据）。
pub fn cleanup_gameplay_resources(
    mut commands: Commands,
    mut audio: Option<ResMut<AudioManager>>,
    mut resource_manager: ResMut<ResourceManager>,
    visuals: Query<Entity, With<GameplayVisual>>,
    mut skin: Option<NonSendMut<SkinRuntime>>,
) {
    // 1. 取走音频租约（**先清空**，无论 AudioManager 是否存在都不残留）
    let lease = resource_manager.audio_lease.take();
    // 2. 停止音频播放（销毁本场铺面轨道，其上全部声音随之停止）；
    //    音频初始化失败时（无设备）Res 不存在 → 跳过停止，仅归还租约
    if let Some(audio) = audio.as_deref_mut() {
        audio.stop_all();
        if let Some(lease) = lease {
            // 释放音频租约 → 引用归零的解码缓存立即回收
            audio.release(&lease);
        }
    }
    // 3. 销毁游玩视觉实体（音符/特效/进度等）
    let mut visual_count = 0usize;
    for e in &visuals {
        commands.entity(e).despawn();
        visual_count += 1;
    }
    // 4. 销毁皮肤槽实体（释放 BGA/纹理 handle）
    let mut slot_count = 0usize;
    if let Some(mut rt) = skin.as_deref_mut() {
        let slots = std::mem::take(&mut rt.slots);
        let text_slots = std::mem::take(&mut rt.text_slots);
        slot_count = slots.len() + text_slots.len();
        for e in slots.into_iter().chain(text_slots) {
            commands.entity(e).despawn();
        }
    }
    // 5. 移除游玩资源（drop 触发各自的资源释放）
    commands.remove_resource::<BgaPlayer>();
    commands.remove_resource::<GameplaySession>();
    commands.remove_resource::<GameplayRender>();
    commands.remove_resource::<GameplayData>();
    commands.remove_resource::<JudgeState>();
    commands.remove_resource::<GaugeState>();
    commands.remove_resource::<LaneStates>();
    commands.remove_resource::<GameplayLoadFailed>();

    let s = resource_manager.stats;
    info!(
        "[resources] 游玩资源已释放：音频 {} 路径 / BGA {} 图 {} 视频 / 视觉 {visual_count} 实体 + {slot_count} 皮肤槽",
        s.audio_paths, s.bga_images, s.bga_videos
    );
}
