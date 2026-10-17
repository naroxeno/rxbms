//! 选曲界面：可滚动铺面列表 + 选择框 + 详情面板 + 音频预览。
//!
//! 玩法范围（用户约定）：仅 7k/5k Single，暂不支持 DP（14k）与 pop（9k）。
//!
//! 交互：
//! - `↑` / `↓` / `PageUp` / `PageDown` / `Home` / `End`：移动选择框
//! - 滚轮：滚动列表
//! - 鼠标左键：单击选中；再次点击已选中行 → 进入游玩
//! - `Enter`：进入游玩；`Space`：重播试听；`F5`：后台重扫曲库；`F1`：设置
//!
//! 列表按需渲染：库里几千首时只创建可视区域的十几行（虚拟化），滚动/换选
//! 只重建这些行，避免大曲库下的 UI 实体爆炸。

mod preview;

use std::path::PathBuf;

use bevy::{input::mouse::MouseWheel, prelude::*, ui::Overflow};

use crate::{
    audio::AudioManager,
    core::{state::AppState, UiFont},
    database::{SongMeta, SongsDb},
    gameplay::chart::SelectedChart,
    library::{LibraryChanged, LibraryStatus, SongScanner},
};

use preview::SongPreview;

/// 每行高度（逻辑像素）。
const ROW_HEIGHT: f32 = 34.0;
/// 滚动一次滚轮的像素。
const WHEEL_STEP: f32 = 56.0;

/// 应用背景色。
fn bg_app() -> Color {
    Color::srgb(0.055, 0.058, 0.068)
}

/// 列表行背景（交替色）。
fn bg_row(index: usize) -> Color {
    if index.is_multiple_of(2) {
        Color::srgb(0.095, 0.10, 0.115)
    } else {
        Color::srgb(0.11, 0.115, 0.13)
    }
}

/// 选中行背景。
fn bg_selected() -> Color {
    Color::srgb(0.17, 0.26, 0.40)
}

/// 高亮色（选择框/关键数值）。
fn accent() -> Color {
    Color::srgb(0.42, 0.72, 1.0)
}

/// 主文字色。
fn text_main() -> Color {
    Color::srgb(0.93, 0.94, 0.96)
}

/// 次要文字色。
fn text_dim() -> Color {
    Color::srgb(0.60, 0.63, 0.70)
}

/// 选曲界面插件。
pub struct SongSelectPlugin;

impl Plugin for SongSelectPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SelectState>()
            .init_resource::<SongPreview>()
            .add_systems(OnEnter(AppState::SongSelect), enter_song_select)
            .add_systems(
                Update,
                (
                    reload_on_scan,
                    track_viewport,
                    select_input,
                    select_wheel,
                    row_click,
                    preview::preview_tick,
                    rebuild_rows,
                    update_details,
                    update_header,
                )
                    .chain()
                    .run_if(in_state(AppState::SongSelect)),
            )
            .add_systems(OnExit(AppState::SongSelect), exit_song_select);
    }
}

/// 选曲界面 UI 根节点。
#[derive(Component)]
struct SongSelectUi;

/// 列表视口（纵向裁剪，`ComputedNode` 高度用于虚拟化计算）。
#[derive(Component)]
struct ListViewport;

/// 列表内容容器（绝对定位，行按 `top` 摆放）。
#[derive(Component)]
struct ListContent;

/// 铺面行（虚拟化只创建可视行）。
#[derive(Component)]
struct SongRow {
    /// 在 `SelectState::songs` 中的下标。
    index: usize,
}

/// 头部右上角：总数 / 扫描进度。
#[derive(Component)]
struct HeaderStatus;

/// 详情面板文字节点。
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum DetailText {
    /// 标题。
    Title,
    /// 艺术家 / 风格。
    Subtitle,
    /// 数值统计。
    Stats,
    /// 文件路径。
    Path,
    /// 预览状态。
    Preview,
}

/// 选曲界面状态（跨进出界面保留选择与滚动位置）。
#[derive(Resource)]
pub(crate) struct SelectState {
    /// 当前铺面列表（数据库过滤后的活跃、非重复铺面）。
    songs: Vec<SongMeta>,
    /// 选择框下标。
    selected: usize,
    /// 列表滚动位置（逻辑像素）。
    scroll: f32,
    /// 视口高度（逻辑像素），由 `track_viewport` 更新。
    viewport_h: f32,
    /// 需要重建可视行与详情。
    dirty: bool,
}

impl Default for SelectState {
    fn default() -> Self {
        Self {
            songs: Vec::new(),
            selected: 0,
            scroll: 0.0,
            viewport_h: 480.0,
            dirty: true,
        }
    }
}

impl SelectState {
    /// 当前选中铺面。
    fn selected_meta(&self) -> Option<&SongMeta> {
        self.songs.get(self.selected)
    }

    /// 当前选中铺面的路径（预览目标）。
    pub(crate) fn selected_path(&self) -> Option<PathBuf> {
        self.selected_meta().map(|meta| meta.path.clone())
    }

    /// 当前选中铺面自带的 `#PREVIEW` 预览音源（扫描时已确认文件存在）。
    pub(crate) fn selected_preview(&self) -> Option<PathBuf> {
        self.selected_meta().and_then(|meta| meta.preview.clone())
    }

    /// 选择框夹到有效范围。
    fn clamp_selection(&mut self) {
        if self.songs.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.songs.len() - 1);
        }
    }

    /// 滚动上限（内容高度 - 视口高度，最小 0）。
    fn max_scroll(&self) -> f32 {
        (self.songs.len() as f32 * ROW_HEIGHT - self.viewport_h).max(0.0)
    }

    /// 滚动位置夹到有效范围。
    fn clamp_scroll(&mut self) {
        let max = self.max_scroll();
        self.scroll = self.scroll.clamp(0.0, max);
    }

    /// 让选择框处在可视区域内。
    fn ensure_visible(&mut self) {
        let top = self.selected as f32 * ROW_HEIGHT;
        let bottom = top + ROW_HEIGHT;
        if top < self.scroll {
            self.scroll = top;
        } else if bottom > self.scroll + self.viewport_h {
            self.scroll = bottom - self.viewport_h;
        }
        self.clamp_scroll();
    }

    /// 用数据库内容刷新列表，尽量保留当前选中项。
    fn reload(&mut self, db: &SongsDb) {
        let keep = self.selected_path();
        self.songs = db.list_songs().unwrap_or_default();
        if let Some(keep) = keep
            && let Some(index) = self.songs.iter().position(|meta| meta.path == keep)
        {
            self.selected = index;
        }
        self.clamp_selection();
        self.ensure_visible();
        self.dirty = true;
    }
}

/// 进入选曲界面：刷新列表并搭建界面骨架。
fn enter_song_select(
    mut commands: Commands,
    mut state: ResMut<SelectState>,
    db: Res<SongsDb>,
    ui_font: Res<UiFont>,
) {
    state.reload(&db);
    state.dirty = true;
    info!("[song-select] 进入选曲界面（{} 个铺面）", state.songs.len());
    spawn_ui(&mut commands, &ui_font);
}

/// 搭建界面骨架：头部 + 列表视口 + 详情面板 + 底部提示。
fn spawn_ui(commands: &mut Commands, ui_font: &UiFont) {
    let font = ui_font.0.clone();
    commands
        .spawn((
            SongSelectUi,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                ..default()
            },
            BackgroundColor(bg_app()),
        ))
        .with_children(|root| {
            // ---- 头部 ----
            root.spawn(Node {
                height: Val::Px(64.0),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                padding: UiRect::axes(Val::Px(28.0), Val::Px(0.0)),
                ..default()
            })
            .with_children(|header| {
                header.spawn((
                    Text::new("选 曲"),
                    TextFont {
                        font: font.clone().into(),
                        font_size: FontSize::Px(26.0),
                        ..default()
                    },
                    TextColor(accent()),
                ));
                header.spawn((
                    HeaderStatus,
                    Text::new(""),
                    TextFont {
                        font: font.clone().into(),
                        font_size: FontSize::Px(14.0),
                        ..default()
                    },
                    TextColor(text_dim()),
                ));
            });

            // ---- 主体：左列表 + 右详情 ----
            root.spawn(Node {
                flex_grow: 1.0,
                flex_direction: FlexDirection::Row,
                padding: UiRect::axes(Val::Px(20.0), Val::Px(0.0)),
                column_gap: Val::Px(16.0),
                min_height: Val::Px(0.0),
                ..default()
            })
            .with_children(|body| {
                // 列表卡片
                body.spawn((
                    Node {
                        flex_grow: 1.0,
                        flex_direction: FlexDirection::Column,
                        min_width: Val::Px(0.0),
                        border: UiRect::all(Val::Px(1.0)),
                        border_radius: BorderRadius::all(Val::Px(6.0)),
                        ..default()
                    },
                    BorderColor::all(Color::srgb(0.16, 0.17, 0.20)),
                    BackgroundColor(Color::srgb(0.075, 0.078, 0.09)),
                ))
                .with_children(|list| {
                    list.spawn((
                        ListViewport,
                        Node {
                            flex_grow: 1.0,
                            min_height: Val::Px(0.0),
                            overflow: Overflow::clip(),
                            ..default()
                        },
                    ))
                    .with_children(|viewport| {
                        viewport.spawn((
                            ListContent,
                            Node {
                                position_type: PositionType::Absolute,
                                width: Val::Percent(100.0),
                                height: Val::Percent(100.0),
                                ..default()
                            },
                        ));
                    });
                });

                // 详情面板
                body.spawn((
                    Node {
                        width: Val::Px(430.0),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(10.0),
                        padding: UiRect::all(Val::Px(18.0)),
                        border: UiRect::all(Val::Px(1.0)),
                        border_radius: BorderRadius::all(Val::Px(6.0)),
                        ..default()
                    },
                    BorderColor::all(Color::srgb(0.16, 0.17, 0.20)),
                    BackgroundColor(Color::srgb(0.085, 0.088, 0.10)),
                ))
                .with_children(|details| {
                    detail_text(details, &font, DetailText::Title, 22.0, text_main());
                    detail_text(details, &font, DetailText::Subtitle, 14.0, accent());
                    // 分隔线
                    details.spawn((
                        Node {
                            height: Val::Px(1.0),
                            width: Val::Percent(100.0),
                            ..default()
                        },
                        BackgroundColor(Color::srgb(0.16, 0.17, 0.20)),
                    ));
                    detail_text(details, &font, DetailText::Stats, 15.0, text_main());
                    detail_text(details, &font, DetailText::Path, 12.0, text_dim());
                    details.spawn(Node {
                        flex_grow: 1.0,
                        ..default()
                    });
                    detail_text(details, &font, DetailText::Preview, 14.0, text_dim());
                });
            });

            // ---- 底部提示 ----
            root.spawn((
                Text::new(
                    "↑↓ 选择   PageUp/PageDown 翻页   滚轮 滚动   点击 选中 / 再点开始   \
                     Enter 开始   Space 试听   F5 刷新   F1 设置",
                ),
                TextFont {
                    font: font.clone().into(),
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
                TextColor(text_dim()),
                Node {
                    height: Val::Px(44.0),
                    padding: UiRect::axes(Val::Px(28.0), Val::Px(0.0)),
                    ..default()
                },
            ));
        });
}

/// 生成详情面板的一行文字。
fn detail_text(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    kind: DetailText,
    size: f32,
    color: Color,
) {
    parent.spawn((
        kind,
        Text::new(""),
        TextFont {
            font: font.clone().into(),
            font_size: FontSize::Px(size),
            ..default()
        },
        TextColor(color),
        Node {
            max_width: Val::Percent(100.0),
            ..default()
        },
    ));
}

/// 曲库扫描完成后重新查询列表（保留选中项）。
fn reload_on_scan(
    mut changed: MessageReader<LibraryChanged>,
    mut state: ResMut<SelectState>,
    db: Res<SongsDb>,
) {
    let mut latest = None;
    for event in changed.read() {
        latest = Some(event.report);
    }
    let Some(report) = latest else {
        return;
    };
    info!("[song-select] 扫描完成，刷新列表: {report}");
    state.reload(&db);
}

/// 记录列表视口高度，变化时重算虚拟化范围。
fn track_viewport(
    viewport: Query<&ComputedNode, With<ListViewport>>,
    mut state: ResMut<SelectState>,
) {
    let Ok(node) = viewport.single() else {
        return;
    };
    let height = node.size.y * node.inverse_scale_factor;
    if height > 1.0 && (height - state.viewport_h).abs() > 0.5 {
        state.viewport_h = height;
        state.clamp_scroll();
        state.dirty = true;
    }
}

/// 键盘：移动选择框 / 进入游玩 / 刷新 / 设置。
fn select_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<SelectState>,
    mut commands: Commands,
    mut next: ResMut<NextState<AppState>>,
    mut audio: ResMut<AudioManager>,
    mut preview: ResMut<SongPreview>,
    scanner: Option<Res<SongScanner>>,
) {
    if keys.just_pressed(KeyCode::F5)
        && let Some(scanner) = scanner.as_deref()
    {
        info!("[song-select] 手动重扫铺面库");
        scanner.request_scan_all();
    }
    if keys.just_pressed(KeyCode::F1) {
        NextState::set_if_neq(&mut next, AppState::Settings);
        return;
    }
    if state.songs.is_empty() {
        return;
    }

    let len = state.songs.len();
    let page = ((state.viewport_h / ROW_HEIGHT).floor() as usize).max(1);
    let mut moved = false;
    if keys.just_pressed(KeyCode::ArrowDown) {
        state.selected = (state.selected + 1).min(len - 1);
        moved = true;
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        state.selected = state.selected.saturating_sub(1);
        moved = true;
    }
    if keys.just_pressed(KeyCode::PageDown) {
        state.selected = (state.selected + page).min(len - 1);
        moved = true;
    }
    if keys.just_pressed(KeyCode::PageUp) {
        state.selected = state.selected.saturating_sub(page);
        moved = true;
    }
    if keys.just_pressed(KeyCode::Home) {
        state.selected = 0;
        moved = true;
    }
    if keys.just_pressed(KeyCode::End) {
        state.selected = len - 1;
        moved = true;
    }
    if moved {
        state.ensure_visible();
        state.dirty = true;
    }
    if keys.just_pressed(KeyCode::Enter) {
        launch(&mut commands, &mut next, &mut audio, &mut preview, &state);
    }
}

/// 滚轮滚动列表。
fn select_wheel(mut wheel: MessageReader<MouseWheel>, mut state: ResMut<SelectState>) {
    let mut delta = 0.0;
    for event in wheel.read() {
        delta += event.y;
    }
    if delta.abs() < f32::EPSILON {
        return;
    }
    state.scroll -= delta * WHEEL_STEP;
    state.clamp_scroll();
    state.dirty = true;
}

/// 鼠标：单击选中，点击已选中行则进入游玩。
#[allow(clippy::type_complexity)]
fn row_click(
    mut state: ResMut<SelectState>,
    mut commands: Commands,
    mut next: ResMut<NextState<AppState>>,
    mut audio: ResMut<AudioManager>,
    mut preview: ResMut<SongPreview>,
    rows: Query<(&Interaction, &SongRow), (Changed<Interaction>, With<Button>)>,
) {
    let mut clicked = None;
    for (interaction, row) in &rows {
        if *interaction == Interaction::Pressed {
            clicked = Some(row.index);
        }
    }
    let Some(index) = clicked else {
        return;
    };
    if index == state.selected {
        launch(&mut commands, &mut next, &mut audio, &mut preview, &state);
        return;
    }
    state.selected = index.min(state.songs.len().saturating_sub(1));
    state.ensure_visible();
    state.dirty = true;
}

/// 进入游玩：写入 `SelectedChart` 并切到 Gameplay。
fn launch(
    commands: &mut Commands,
    next: &mut NextState<AppState>,
    audio: &mut AudioManager,
    preview: &mut SongPreview,
    state: &SelectState,
) {
    let Some(meta) = state.selected_meta() else {
        return;
    };
    let title = meta
        .title
        .clone()
        .unwrap_or_else(|| meta.file_name.clone());
    info!("[song-select] 开始游玩: {title}");
    // 预览与游玩互斥：先彻底停掉预览（含音频租约）
    preview.stop(audio);
    audio.stop_all();
    commands.insert_resource(SelectedChart {
        path: meta.path.clone(),
        title,
    });
    NextState::set_if_neq(next, AppState::Gameplay);
}

/// 重建可视行（虚拟化）：只渲染视口内的十几行。
fn rebuild_rows(
    mut commands: Commands,
    mut state: ResMut<SelectState>,
    ui_font: Res<UiFont>,
    content: Query<Entity, With<ListContent>>,
) {
    if !state.dirty {
        return;
    }
    let Ok(content) = content.single() else {
        return;
    };
    state.dirty = false;

    commands.entity(content).despawn_children();
    commands.entity(content).with_children(|parent| {
        if state.songs.is_empty() {
            parent.spawn((
                Text::new("曲库为空：按 F1 到设置里添加铺面目录，再按 F5 扫描"),
                TextFont {
                    font: ui_font.0.clone().into(),
                    font_size: FontSize::Px(15.0),
                    ..default()
                },
                TextColor(text_dim()),
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(24.0),
                    left: Val::Px(20.0),
                    ..default()
                },
            ));
            return;
        }

        let first = (state.scroll / ROW_HEIGHT).floor().max(0.0) as usize;
        let visible = ((state.viewport_h / ROW_HEIGHT).ceil() as usize + 2).max(1);
        let end = (first + visible).min(state.songs.len());
        let font = ui_font.0.clone();

        for index in first..end {
            let meta = &state.songs[index];
            let selected = index == state.selected;
            let title = meta.title.clone().unwrap_or_else(|| meta.file_name.clone());
            let artist = meta.artist.clone().unwrap_or_default();
            let level = meta
                .play_level
                .map_or_else(|| String::from("?"), |level| level.to_string());
            let top = index as f32 * ROW_HEIGHT - state.scroll;

            parent
                .spawn((
                    SongRow { index },
                    Button,
                    Node {
                        position_type: PositionType::Absolute,
                        top: Val::Px(top),
                        left: Val::Px(0.0),
                        width: Val::Percent(100.0),
                        height: Val::Px(ROW_HEIGHT - 2.0),
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        padding: UiRect::axes(Val::Px(10.0), Val::Px(0.0)),
                        column_gap: Val::Px(10.0),
                        border: if selected {
                            UiRect::all(Val::Px(2.0))
                        } else {
                            UiRect::all(Val::Px(0.0))
                        },
                        ..default()
                    },
                    BackgroundColor(if selected {
                        bg_selected()
                    } else {
                        bg_row(index)
                    }),
                    BorderColor::all(accent()),
                ))
                .with_children(|row| {
                    row.spawn((
                        Text::new(format!("{:03}", index + 1)),
                        TextFont {
                            font: font.clone().into(),
                            font_size: FontSize::Px(13.0),
                            ..default()
                        },
                        TextColor(if selected { accent() } else { text_dim() }),
                        Node {
                            width: Val::Px(44.0),
                            ..default()
                        },
                    ));
                    row.spawn((
                        Text::new(title),
                        TextFont {
                            font: font.clone().into(),
                            font_size: FontSize::Px(15.0),
                            ..default()
                        },
                        TextColor(if selected { text_main() } else { text_dim() }),
                        Node {
                            flex_grow: 1.0,
                            ..default()
                        },
                    ));
                    if !artist.is_empty() {
                        row.spawn((
                                Text::new(artist),
                            TextFont {
                                font: font.clone().into(),
                                font_size: FontSize::Px(13.0),
                                ..default()
                            },
                            TextColor(text_dim()),
                        ));
                    }
                    row.spawn((
                        Text::new(format!("Lv.{level}")),
                        TextFont {
                            font: font.clone().into(),
                            font_size: FontSize::Px(14.0),
                            ..default()
                        },
                        TextColor(if selected { accent() } else { text_dim() }),
                        Node {
                            width: Val::Px(52.0),
                            ..default()
                        },
                    ));
                    row.spawn((
                        Text::new(format!("{} 音符", meta.playable_count)),
                        TextFont {
                            font: font.clone().into(),
                            font_size: FontSize::Px(13.0),
                            ..default()
                        },
                        TextColor(text_dim()),
                        Node {
                            width: Val::Px(88.0),
                            ..default()
                        },
                    ));
                });
        }
    });
}

/// 详情面板：随选中项更新。
fn update_details(
    state: Res<SelectState>,
    preview: Res<SongPreview>,
    mut texts: Query<(&DetailText, &mut Text)>,
) {
    let meta = state.selected_meta();
    let preview_label = preview.status().label();
    for (kind, mut text) in &mut texts {
        let value = match kind {
            DetailText::Title => meta
                .map(|meta| meta.title.clone().unwrap_or_else(|| meta.file_name.clone()))
                .unwrap_or_else(|| String::from("（没有可用铺面）")),
            DetailText::Subtitle => meta
                .map(|meta| {
                    let artist = meta.artist.as_deref().unwrap_or("未知艺术家");
                    let genre = meta.genre.as_deref().unwrap_or("?");
                    format!("{artist}   ·   {genre}")
                })
                .unwrap_or_default(),
            DetailText::Stats => meta
                .map(|meta| {
                    format!(
                        "等级 Lv.{}\n初始 BPM {}\n音符 {}（可玩 {}）\nBGM 事件 {} · 小节 {}",
                        meta.play_level
                            .map_or_else(|| String::from("?"), |level| level.to_string()),
                        meta.initial_bpm
                            .map_or_else(|| String::from("?"), |bpm| format!("{bpm:.0}")),
                        meta.note_count,
                        meta.playable_count,
                        meta.bgm_count,
                        meta.measure_count,
                    )
                })
                .unwrap_or_default(),
            DetailText::Path => meta
                .map(|meta| meta.path.display().to_string())
                .unwrap_or_default(),
            DetailText::Preview => preview_label.clone(),
        };
        if text.0 != value {
            text.0 = value;
        }
    }
}

/// 头部右上角：总数 + 扫描进度。
fn update_header(
    state: Res<SelectState>,
    status: Res<LibraryStatus>,
    mut header: Query<&mut Text, With<HeaderStatus>>,
) {
    let suffix = if status.scanning {
        format!("　扫描中 {}/{}", status.done, status.total)
    } else {
        String::new()
    };
    let value = format!("共 {} 首{suffix}", state.songs.len());
    for mut text in &mut header {
        if text.0 != value {
            text.0 = value.clone();
        }
    }
}

/// 离开选曲界面：清理界面与预览音频。
fn exit_song_select(
    mut commands: Commands,
    mut audio: ResMut<AudioManager>,
    mut preview: ResMut<SongPreview>,
    roots: Query<Entity, With<SongSelectUi>>,
) {
    for root in &roots {
        commands.entity(root).despawn();
    }
    preview.stop(&mut audio);
    // 无论切向哪个界面，离开选曲都停止选曲音频
    audio.stop_all();
}
