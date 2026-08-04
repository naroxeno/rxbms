//! 应用全局状态机。
//!
//! 所有界面（选曲 / 游玩 / 结算 / 设置）都是该状态机的一个状态，
//! 界面切换通过 [`NextState<AppState>`] 触发。

use bevy::prelude::*;

/// 全局应用状态。
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    /// 启动阶段：加载全局资源、扫描铺面库，完成后自动进入 [`AppState::SongSelect`]。
    #[default]
    Startup,
    /// 选曲界面。
    SongSelect,
    /// 游玩界面（下落式音符 + 判定）。
    Gameplay,
    /// 结算界面（成绩展示 + 记录写入）。
    Result,
    /// 设置界面（键位 / 音频 / 显示）。
    Settings,
}

impl AppState {
    /// 状态的调试展示名。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Startup => "Startup",
            Self::SongSelect => "SongSelect",
            Self::Gameplay => "Gameplay",
            Self::Result => "Result",
            Self::Settings => "Settings",
        }
    }
}
