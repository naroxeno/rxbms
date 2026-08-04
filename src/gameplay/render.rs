//! 音符判定状态数据（Lua 皮肤渲染已接管视觉，本模块仅保留判定所需数据）。
//!
//! `NoteRender` 是每音符判定状态（`consumed`）的唯一数据源（bms-rs 无 per-note
//! 状态），5 处判定逻辑与 Lua 皮肤同步都依赖它。`GameplayRender.note_entities`
//! 提供音符实体索引（与 `LoadedChart.notes` 对齐）。

use bevy::prelude::*;

use super::chart::LoadedChart;

/// 游玩界面视觉实体标记（OnExit 统一清理；Lua 皮肤槽不标记，由皮肤模块管理）。
#[derive(Component)]
pub struct GameplayVisual;

/// 音符实体的判定状态（`consumed` 为 per-note 判定数据源；位置/长度读 `LoadedChart.notes`）。
#[derive(Component)]
pub struct NoteRender {
    /// 已判定（隐藏）。
    pub consumed: bool,
}

/// 音符实体索引（判定状态数据源，无渲染职责）。
#[derive(Resource)]
pub struct GameplayRender {
    /// 音符实体（与 `LoadedChart.notes` 对齐）。
    pub note_entities: Vec<Entity>,
}

impl GameplayRender {
    /// 生成音符判定状态实体（无 sprite/transform——视觉由 Lua 皮肤渲染）。
    pub fn spawn(commands: &mut Commands, loaded: &LoadedChart) -> Self {
        let mut note_entities = Vec::with_capacity(loaded.notes.len());
        for _note in &loaded.notes {
            let entity = commands
                .spawn((
                    super::GameplayVisual,
                    NoteRender { consumed: false },
                ))
                .id();
            note_entities.push(entity);
        }
        Self { note_entities }
    }
}
