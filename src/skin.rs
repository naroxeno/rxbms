//! 皮肤模块：beatoraja Lua 皮肤运行时（加载/渲染）。
//!
//! 全部游玩视觉（背景/轨道/判定线/音符/数字/文本/特效）由 Lua 皮肤渲染，
//! 硬编码渲染已移除（见 gameplay/render.rs 仅保留判定状态数据）。
//! 皮肤加载失败时回退为纯色背景 + 错误提示（runtime::load_lua_skin）。

pub mod lua;
pub mod material;
pub mod model;
pub mod render;
pub mod runtime;
pub mod state;

use bevy::prelude::*;

use crate::core::state::AppState;

/// 皮肤插件：加载 Lua 皮肤（Startup），游玩中渲染（Update），退出时隐藏皮肤槽。
pub struct SkinPlugin;

impl Plugin for SkinPlugin {
    fn build(&self, app: &mut App) {
        // 皮肤特效材质（black-key 抠像 / RGB 通道重排）
        app.add_plugins(bevy::sprite_render::Material2dPlugin::<material::SkinFxMaterial>::default());
        // 皮肤按谱面模式加载（5K→Play5 / 7K→Play7），须在 setup_gameplay 之后
        app.add_systems(
            OnEnter(AppState::Gameplay),
            runtime::load_lua_skin.after(crate::gameplay::setup_gameplay),
        )
        .add_systems(
            Update,
            runtime::apply_skin_frame
                .after(crate::gameplay::SkinSyncSet)
                .run_if(in_state(AppState::Gameplay)),
        )
        .add_systems(OnExit(AppState::Gameplay), runtime::hide_skin_slots);
    }
}
