//! 游玩记录模块：成绩持久化与历史查询。
//!
//! Phase 0 为空壳，后续实现：
//! - 每次游玩结束后将成绩序列化为 JSON 保存到 `~/.rxbms/records/`；
//! - 供选曲界面展示历史最佳成绩。

use bevy::prelude::*;

use crate::core::state::AppState;

/// 游玩记录插件。
pub struct RecordPlugin;

impl Plugin for RecordPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Result), save_record);
    }
}

fn save_record() {
    info!("[record] 写入游玩记录… (TODO)");
}
