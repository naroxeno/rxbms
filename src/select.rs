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
    library::{LibraryChanged, LibraryStatus, SongScanner},
};

/// 选曲界面插件。
pub struct SongSelectPlugin;

impl Plugin for SongSelectPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::SongSelect), enter_song_select)
            .add_systems(
                Update,
                (song_row_click, song_select_keys, rebuild_after_scan)
                    .run_if(in_state(AppState::SongSelect)),
            )
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

fn enter_song_select(
    mut commands: Commands,
    db: Res<SongsDb>,
    ui_font: Res<UiFont>,
    status: Res<LibraryStatus>,
) {
    let songs = db.list_songs().unwrap_or_default();
    info!(
        "[song-select] 进入选曲界面（{} 个铺面{}，点击进入游玩）",
        songs.len(),
        if status.scanning {
            format!("，后台扫描中 {}/{}", status.done, status.total)
        } else {
            String::new()
        }
    );
    spawn_song_list(&mut commands, &db, &ui_font);
}

/// 按数据库当前内容重建列表（数据库查询失败时静默保持空列表）。
fn spawn_song_list(commands: &mut Commands, db: &SongsDb, ui_font: &UiFont) {
    let songs = db.list_songs().unwrap_or_default();
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

/// 后台扫描完成后重建铺面列表（保留当前选中项由后续选曲界面实现接管）。
fn rebuild_after_scan(
    mut commands: Commands,
    mut changed: MessageReader<LibraryChanged>,
    db: Res<SongsDb>,
    ui_font: Res<UiFont>,
    roots: Query<Entity, With<SongListUi>>,
) {
    let mut latest = None;
    for event in changed.read() {
        latest = Some(event.report);
    }
    let Some(report) = latest else {
        return;
    };
    info!("[song-select] 扫描完成，刷新列表: {report}");
    for root in &roots {
        commands.entity(root).despawn();
    }
    spawn_song_list(&mut commands, &db, &ui_font);
}

/// 选曲界面按键：F5 手动重扫（后台执行，唯一的列表刷新入口之一）。
fn song_select_keys(keys: Res<ButtonInput<KeyCode>>, scanner: Option<Res<SongScanner>>) {
    if keys.just_pressed(KeyCode::F5)
        && let Some(scanner) = scanner
    {
        info!("[song-select] 手动重扫铺面库");
        scanner.request_scan_all();
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
