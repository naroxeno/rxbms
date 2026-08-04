//! 选曲界面模块：从 `songs.db` 查询铺面列表，点击进入游玩。
//!
//! 玩法范围（用户约定）：仅 7k/5k Single，暂不支持 DP（14k）与 pop（9k）。
//! TODO：滚动 / 排序 / 筛选、预览面板。

use std::path::PathBuf;

use bevy::prelude::*;

use crate::{
    audio::AudioManager,
    core::{state::AppState, UiFont},
    database::SongsDb,
    gameplay::chart::SelectedChart,
};

/// 选曲界面插件。
pub struct SongSelectPlugin;

impl Plugin for SongSelectPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::SongSelect), enter_song_select)
            .add_systems(Update, song_row_click.run_if(in_state(AppState::SongSelect)))
            .add_systems(OnExit(AppState::SongSelect), exit_song_select);
    }
}

/// 列表 UI 根节点标记，OnExit 时整体清理。
#[derive(Component)]
struct SongListUi;

/// 铺面行：点击进入游玩。
#[derive(Component)]
struct SongRow {
    path: PathBuf,
    title: String,
}

fn enter_song_select(mut commands: Commands, db: Res<SongsDb>, ui_font: Res<UiFont>) {
    let songs = db.list_songs().unwrap_or_default();
    info!("[song-select] 进入选曲界面（{} 个铺面，点击进入游玩）", songs.len());
    commands
        .spawn((
            SongListUi,
            Node {
                flex_direction: FlexDirection::Column,
                position_type: PositionType::Absolute,
                top: Val::Px(48.0),
                left: Val::Px(16.0),
                width: Val::Px(720.0),
                row_gap: Val::Px(2.0),
                ..default()
            },
        ))
        .with_children(|parent| {
            for meta in &songs {
                parent
                    .spawn((
                        SongRow {
                            path: meta.path.clone(),
                            title: meta
                                .title
                                .clone()
                                .unwrap_or_else(|| meta.file_name.clone()),
                        },
                        Button,
                        Node {
                            width: Val::Px(720.0),
                            height: Val::Px(26.0),
                            align_items: AlignItems::Center,
                            padding: UiRect::axes(Val::Px(8.0), Val::Px(0.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgb(0.12, 0.12, 0.15)),
                        children![(
                            Text::new(meta.list_line()),
                            TextFont {
                                font: ui_font.0.clone().into(),
                                font_size: FontSize::Px(15.0),
                                ..default()
                            },
                            TextColor(Color::srgb(0.9, 0.9, 0.9)),
                        )],
                    ));
            }
        });
}

/// 点击铺面行 → 写入 `SelectedChart` 并进入游玩；离开时停止主界面 BGM。
#[allow(clippy::type_complexity)] // Bevy 系统 Query 参数
fn song_row_click(
    mut commands: Commands,
    mut next: ResMut<NextState<AppState>>,
    mut audio: ResMut<AudioManager>,
    rows: Query<(&Interaction, &SongRow), (Changed<Interaction>, With<Button>)>,
) {
    for (interaction, row) in &rows {
        if *interaction == Interaction::Pressed {
            info!("[song-select] 选择: {}", row.title);
            // 主界面音轨与 gameplay 互斥：进入游玩前停掉选曲 BGM
            audio.stop_menu_bgm();
            commands.insert_resource(SelectedChart {
                path: row.path.clone(),
                title: row.title.clone(),
            });
            NextState::set_if_neq(&mut next, AppState::Gameplay);
        }
    }
}

fn exit_song_select(
    mut commands: Commands,
    mut audio: ResMut<AudioManager>,
    roots: Query<Entity, With<SongListUi>>,
) {
    for root in &roots {
        commands.entity(root).despawn();
    }
    // 无论切向哪个界面，离开选曲都停止主界面 BGM
    audio.stop_menu_bgm();
}
