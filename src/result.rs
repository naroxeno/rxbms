//! 结算界面模块：成绩展示与判定统计。
//!
//! Phase 0 为空壳，后续实现：
//! - 总分、EX 分数与各判定计数展示；
//! - FAST / SLOW 判定分布图与最大连击数；
//! - 返回选曲的入口。

use bevy::prelude::*;

use crate::core::state::AppState;

/// 结算界面插件。
pub struct ResultPlugin;

impl Plugin for ResultPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Result), enter_result);
    }
}

fn enter_result() {
    info!("[result] 进入结算界面… (TODO)");
}
