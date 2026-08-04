//! 设置界面模块：按注册表（大表）动态渲染所有内建设置项 + 铺面文件夹管理。
//!
//! 可拓展性：新增设置项只需在 `core::settings::SettingsRegistry::builtin()`
//! 加一行定义，本界面自动出现对应控件（Bool 开关 / 数值 ± / 枚举循环 / 键位重绑）。

use std::path::PathBuf;

use bevy::{
    input::keyboard::KeyboardInput,
    prelude::*,
};

use crate::{
    core::keybind::KeyBindingsByMode,
    core::settings::{
        SettingCategory, SettingKind, SettingValue, SettingsFile, SettingsRegistry,
        SettingsStore, save_settings_file,
    },
    core::state::AppState,
    core::UiFont,
    database::SongsDb,
};

/// 设置界面插件。
pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SettingsUiState>()
            .add_systems(OnEnter(AppState::Settings), settings_ui_setup)
            .add_systems(OnExit(AppState::Settings), settings_ui_teardown)
            .add_systems(
                Update,
                (settings_actions, refresh_settings_ui)
                    .chain()
                    .run_if(in_state(AppState::Settings)),
            );
    }
}

/// 设置界面的可变状态（输入框内容、渲染同步、重绑）。
#[derive(Resource, Default)]
struct SettingsUiState {
    /// 文件夹输入框中的路径文本。
    input: String,
    /// 输入框是否处于键盘接收状态。
    input_active: bool,
    /// 状态文本（扫描报告 / 错误信息）。
    status: Option<String>,
    /// 当前已渲染的文件夹列表（用于增量刷新）。
    rendered_folders: Vec<PathBuf>,
    /// 正在重新绑定的设置项 id（等待按键）。
    rebind: Option<&'static str>,
    /// 当前已渲染的设置项 id 列表（用于增量刷新）。
    rendered_settings: Vec<&'static str>,
}

// ---------- UI 标记组件 ----------

/// 设置界面根节点。
#[derive(Component)]
struct SettingsUi;

/// 文件夹列表容器。
#[derive(Component)]
struct FolderList;

/// 单行文件夹。
#[derive(Component)]
struct FolderRow;

/// 输入框。
#[derive(Component)]
struct FolderInputBox;

/// 输入框内文本。
#[derive(Component)]
struct FolderInputText;

/// 状态文本。
#[derive(Component)]
struct StatusText;

/// 动态设置项容器（按分类分组）。
#[derive(Component)]
struct SettingsSection;

/// 单行设置项。
#[derive(Component)]
struct SettingRow;

/// 分类标题（重建时与设置行一起清理）。
#[derive(Component)]
struct SettingHeader;

/// 按钮动作。
#[derive(Component)]
enum SettingsAction {
    // 文件夹管理
    FocusInput,
    AddFolder,
    RemoveFolder { path: PathBuf },
    Rescan,
    BackToSelect,
    // 设置项调整（语义随 kind）
    SettingDec { id: &'static str },
    SettingInc { id: &'static str },
    SettingToggle { id: &'static str },
    SettingCycle { id: &'static str },
    SettingRebind { id: &'static str },
}

// ---------- 界面构建 ----------

fn settings_ui_setup(mut commands: Commands, ui_font: Res<UiFont>, mut state: ResMut<SettingsUiState>) {
    // 重置界面状态缓存（避免跨会话残留导致动态区不重建 / 重复渲染）
    *state = SettingsUiState::default();
    info!("[settings] 进入设置界面");
    commands
        .spawn((
            SettingsUi,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(16.0),
                left: Val::Px(16.0),
                width: Val::Px(720.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(8.0),
                padding: UiRect::all(Val::Px(16.0)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.08, 0.08, 0.10)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("设置"),
                TextFont {
                    font: ui_font.0.clone().into(),
                    font_size: FontSize::Px(22.0),
                    ..default()
                },
                TextColor(Color::WHITE),
            ));

            // 设置项（按分类分组，动态刷新）
            parent.spawn((
                SettingsSection,
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(2.0),
                    ..default()
                },
            ));

            // 文件夹管理区
            parent.spawn((
                Text::new("铺面文件夹"),
                TextFont {
                    font: ui_font.0.clone().into(),
                    font_size: FontSize::Px(16.0),
                    ..default()
                },
                TextColor(Color::srgb(0.85, 0.85, 0.9)),
            ));
            parent.spawn((
                FolderList,
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(4.0),
                    ..default()
                },
            ));

            // 输入行：输入框 + 添加按钮
            parent
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(8.0),
                    ..default()
                })
                .with_children(|parent| {
                    parent
                        .spawn((
                            FolderInputBox,
                            Button,
                            SettingsAction::FocusInput,
                            Node {
                                width: Val::Px(420.0),
                                height: Val::Px(32.0),
                                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                                ..default()
                            },
                            BackgroundColor(Color::srgb(0.15, 0.15, 0.18)),
                        ))
                        .with_children(|parent| {
                            parent.spawn((
                                FolderInputText,
                                Text::new(""),
                                TextFont {
                                    font: ui_font.0.clone().into(),
                                    font_size: FontSize::Px(15.0),
                                    ..default()
                                },
                                TextColor(Color::srgb(0.9, 0.9, 0.9)),
                            ));
                        });
                    parent.spawn((
                        Button,
                        SettingsAction::AddFolder,
                        Node {
                            width: Val::Px(72.0),
                            height: Val::Px(32.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        BackgroundColor(Color::srgb(0.2, 0.4, 0.7)),
                        children![(
                            Text::new("添加"),
                            TextFont {
                                font: ui_font.0.clone().into(),
                                font_size: FontSize::Px(15.0),
                                ..default()
                            },
                            TextColor(Color::WHITE),
                        )],
                    ));
                });

            // 操作行：重扫 + 返回
            parent
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(8.0),
                    ..default()
                })
                .with_children(|parent| {
                    parent.spawn((
                        Button,
                        SettingsAction::Rescan,
                        Node {
                            width: Val::Px(110.0),
                            height: Val::Px(32.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        BackgroundColor(Color::srgb(0.3, 0.5, 0.3)),
                        children![(
                            Text::new("重新扫描"),
                            TextFont {
                                font: ui_font.0.clone().into(),
                                font_size: FontSize::Px(15.0),
                                ..default()
                            },
                            TextColor(Color::WHITE),
                        )],
                    ));
                    parent.spawn((
                        Button,
                        SettingsAction::BackToSelect,
                        Node {
                            width: Val::Px(110.0),
                            height: Val::Px(32.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        BackgroundColor(Color::srgb(0.4, 0.35, 0.3)),
                        children![(
                            Text::new("返回选曲"),
                            TextFont {
                                font: ui_font.0.clone().into(),
                                font_size: FontSize::Px(15.0),
                                ..default()
                            },
                            TextColor(Color::WHITE),
                        )],
                    ));
                });

            // 状态文本
            parent.spawn((
                StatusText,
                Text::new(""),
                TextFont {
                    font: ui_font.0.clone().into(),
                    font_size: FontSize::Px(14.0),
                    ..default()
                },
                TextColor(Color::srgb(0.7, 0.8, 0.7)),
            ));
        });
}

fn settings_ui_teardown(mut commands: Commands, roots: Query<Entity, With<SettingsUi>>) {
    for root in &roots {
        commands.entity(root).despawn();
    }
}

// ---------- 交互 ----------

/// 按键的可读名（"KeyA" → "A"）。
fn keycode_label(code: KeyCode) -> String {
    let s = format!("{code:?}");
    s.strip_prefix("Key").unwrap_or(&s).to_string()
}

/// 设置值显示文本。
fn value_label(value: &SettingValue) -> String {
    match value {
        SettingValue::Bool(b) => if *b { "开".into() } else { "关".into() },
        SettingValue::Int(i) => i.to_string(),
        SettingValue::Float(f) => format!("{f:.2}"),
        SettingValue::String(s) => s.clone(),
        SettingValue::KeyCode(k) => keycode_label(*k),
    }
}

/// 保存设置（写入 config.json）并重建键位绑定。
fn persist_settings(
    store: &SettingsStore,
    bindings: &mut KeyBindingsByMode,
    state: &mut SettingsUiState,
) {
    let file = SettingsFile {
        settings: Some(store.all().clone()),
    };
    match save_settings_file(&file) {
        Ok(()) => {
            *bindings = KeyBindingsByMode::from_store(store);
            state.status = Some("设置已保存".into());
        }
        Err(e) => state.status = Some(format!("保存失败: {e}")),
    }
}

/// 处理按钮点击、键位重绑捕获与输入框键盘事件。
#[allow(clippy::too_many_arguments)] // Bevy 系统参数
#[allow(clippy::type_complexity)] // Bevy 系统 Query 参数
fn settings_actions(
    db: Res<SongsDb>,
    mut bindings: ResMut<KeyBindingsByMode>,
    registry: Res<SettingsRegistry>,
    mut store: ResMut<SettingsStore>,
    mut state: ResMut<SettingsUiState>,
    mut next: ResMut<NextState<AppState>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut kb: MessageReader<KeyboardInput>,
    buttons: Query<(&Interaction, &SettingsAction), (Changed<Interaction>, With<Button>)>,
) {
    // 1. 键位重绑捕获：优先于输入框
    if let Some(id) = state.rebind {
        for event in kb.read() {
            if !event.state.is_pressed() || event.key_code == KeyCode::Escape {
                continue;
            }
            // 冲突处理：若该键码已被其他键位设置项占用，先清除旧绑定
            for def in &registry.defs {
                if def.kind == SettingKind::KeyCode
                    && def.id != id
                    && store.get_keycode(def.id, KeyCode::Space) == event.key_code
                {
                    store.set(def.id, def.default.clone());
                }
            }
            store.set(id, SettingValue::KeyCode(event.key_code));
            state.status = Some(format!(
                "已绑定 {} → {}",
                registry.get(id).map_or(id, |d| d.name),
                keycode_label(event.key_code)
            ));
            state.rebind = None;
            state.rendered_settings.clear();
            persist_settings(&store, &mut bindings, &mut state);
            break;
        }
        if keys.just_pressed(KeyCode::Escape) {
            state.rebind = None;
        }
    } else if state.input_active {
        // 输入框激活时收集键盘文本
        if keys.just_pressed(KeyCode::Escape) {
            state.input_active = false;
        } else if keys.just_pressed(KeyCode::Enter) {
            add_folder_from_input(&db, &mut state);
        } else {
            for event in kb.read() {
                if !event.state.is_pressed() {
                    continue;
                }
                if event.key_code == KeyCode::Backspace {
                    state.input.pop();
                    continue;
                }
                if let Some(text) = &event.text {
                    state.input.push_str(text);
                }
            }
        }
    }

    // 2. 按钮点击
    for (interaction, action) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match action {
            SettingsAction::FocusInput => {
                state.input_active = !state.input_active;
            }
            SettingsAction::AddFolder => add_folder_from_input(&db, &mut state),
            SettingsAction::RemoveFolder { path } => {
                match db.remove_folder(path) {
                    Ok(()) => state.status = Some(format!("已移除: {}", path.display())),
                    Err(e) => state.status = Some(format!("移除失败: {e}")),
                }
                state.rendered_folders.clear();
            }
            SettingsAction::Rescan => run_scan(&db, &mut state),
            SettingsAction::BackToSelect => {
                NextState::set_if_neq(&mut next, AppState::SongSelect);
            }
            // 设置项调整
            SettingsAction::SettingDec { id } => {
                adjust_number(&registry, &mut store, id, false);
                state.rendered_settings.clear();
                persist_settings(&store, &mut bindings, &mut state);
            }
            SettingsAction::SettingInc { id } => {
                adjust_number(&registry, &mut store, id, true);
                state.rendered_settings.clear();
                persist_settings(&store, &mut bindings, &mut state);
            }
            SettingsAction::SettingToggle { id } => {
                let cur = store.get_bool(id, false);
                store.set(id, SettingValue::Bool(!cur));
                state.rendered_settings.clear();
                persist_settings(&store, &mut bindings, &mut state);
            }
            SettingsAction::SettingCycle { id } => {
                cycle_enum(&registry, &mut store, id);
                state.rendered_settings.clear();
                persist_settings(&store, &mut bindings, &mut state);
            }
            SettingsAction::SettingRebind { id } => {
                state.input_active = false;
                state.rebind = Some(*id);
                let name = registry.get(id).map_or(*id, |d| d.name);
                state.status = Some(format!("按下新键以绑定 {name}…"));
            }
        }
    }
}

/// 数值设置 ± 调整（Int/Float）。
fn adjust_number(registry: &SettingsRegistry, store: &mut SettingsStore, id: &'static str, inc: bool) {
    let Some(def) = registry.get(id) else { return };
    match def.kind {
        SettingKind::Int { min, max, step } => {
            let cur = store.get_int(id, 0);
            let next = (cur + if inc { step } else { -step }).clamp(min, max);
            store.set(id, SettingValue::Int(next));
        }
        SettingKind::Float { min, max, step } => {
            let cur = store.get_float(id, 0.0);
            let next = (cur + if inc { step } else { -step }).clamp(min, max);
            store.set(id, SettingValue::Float(next));
        }
        _ => {}
    }
}

/// 枚举设置循环切换。
fn cycle_enum(registry: &SettingsRegistry, store: &mut SettingsStore, id: &'static str) {
    let Some(def) = registry.get(id) else { return };
    if def.options.is_empty() {
        return;
    }
    let cur = store.value(id);
    let cur_idx = cur
        .and_then(|v| def.options.iter().position(|(_, o)| o == v))
        .unwrap_or(0);
    let next = &def.options[(cur_idx + 1) % def.options.len()];
    store.set(id, next.1.clone());
}

/// 从输入框内容添加文件夹并立即扫描。
fn add_folder_from_input(db: &SongsDb, state: &mut SettingsUiState) {
    let input = state.input.trim();
    if input.is_empty() {
        state.status = Some("请输入文件夹路径".into());
        return;
    }
    let path = PathBuf::from(input);
    if !path.is_dir() {
        state.status = Some(format!("目录不存在: {}", path.display()));
        return;
    }
    match db.add_folder(&path) {
        Ok(()) => {
            state.input.clear();
            run_scan(db, state);
        }
        Err(e) => state.status = Some(format!("添加失败: {e}")),
    }
}

/// 执行增量扫描并把报告写入状态文本。
fn run_scan(db: &SongsDb, state: &mut SettingsUiState) {
    match db.scan() {
        Ok(report) => {
            state.status = Some(report.to_string());
            if report.failed > 0 {
                warn!("[settings] 扫描完成但有失败: {report}");
            } else {
                info!("[settings] {report}");
            }
        }
        Err(e) => {
            state.status = Some(format!("扫描失败: {e}"));
            error!("[settings] 扫描失败: {e}");
        }
    }
    state.rendered_folders.clear();
}

// ---------- 渲染刷新 ----------

/// 每帧同步 UI：设置项（按分类分组）、文件夹列表、输入框、状态文本。
#[allow(clippy::too_many_arguments)] // Bevy 系统参数
fn refresh_settings_ui(
    db: Res<SongsDb>,
    registry: Res<SettingsRegistry>,
    store: Res<SettingsStore>,
    mut state: ResMut<SettingsUiState>,
    ui_font: Res<UiFont>,
    mut commands: Commands,
    folder_rows: Query<Entity, With<FolderRow>>,
    list_q: Query<Entity, With<FolderList>>,
    setting_rows: Query<Entity, With<SettingRow>>,
    setting_headers: Query<Entity, With<SettingHeader>>,
    section_q: Query<Entity, With<SettingsSection>>,
    mut input_text: Query<&mut Text, (With<FolderInputText>, Without<StatusText>)>,
    mut status_text: Query<&mut Text, (With<StatusText>, Without<FolderInputText>)>,
    mut input_box: Query<&mut BackgroundColor, (With<FolderInputBox>, Without<FolderRow>)>,
) {
    // 输入框激活高亮
    for mut bg in &mut input_box {
        *bg = if state.input_active {
            BackgroundColor(Color::srgb(0.2, 0.25, 0.35))
        } else {
            BackgroundColor(Color::srgb(0.15, 0.15, 0.18))
        };
    }

    // 输入框文本（激活时显示光标）
    for mut t in &mut input_text {
        let cursor = if state.input_active { "|" } else { "" };
        t.0 = format!("{}{}", state.input, cursor);
    }

    // 状态文本
    if let Some(status) = &state.status {
        for mut t in &mut status_text {
            t.0 = status.clone();
        }
    }

    // 设置项：按注册表渲染（id 列表变化时重建）
    let ids: Vec<&'static str> = registry.defs.iter().map(|d| d.id).collect();
    if ids != state.rendered_settings {
        for row in &setting_rows {
            commands.entity(row).despawn();
        }
        for header in &setting_headers {
            commands.entity(header).despawn();
        }
        let Ok(section) = section_q.single() else {
            return;
        };
        let entries: Vec<(usize, &crate::core::settings::SettingDef)> =
            registry.defs.iter().enumerate().collect();
        commands.entity(section).with_children(|parent| {
            let mut last_cat: Option<SettingCategory> = None;
            for (_, def) in entries {
                // 分类标题（切换时插入）
                if last_cat != Some(def.category) {
                    parent.spawn((
                        SettingHeader,
                        Text::new(def.category.label()),
                        TextFont {
                            font: ui_font.0.clone().into(),
                            font_size: FontSize::Px(15.0),
                            ..default()
                        },
                        TextColor(Color::srgb(0.6, 0.8, 0.9)),
                    ));
                    last_cat = Some(def.category);
                }
                let value = store.value(def.id).cloned().unwrap_or_else(|| def.default.clone());
                parent
                    .spawn((
                        SettingRow,
                        Node {
                            flex_direction: FlexDirection::Row,
                            column_gap: Val::Px(8.0),
                            align_items: AlignItems::Center,
                            ..default()
                        },
                    ))
                    .with_children(|parent| {
                        parent.spawn((
                            Text::new(def.name),
                            TextFont {
                                font: ui_font.0.clone().into(),
                                font_size: FontSize::Px(14.0),
                                ..default()
                            },
                            TextColor(Color::srgb(0.85, 0.85, 0.85)),
                        ));
                        parent.spawn((
                            Text::new(
                                def.options
                                    .iter()
                                    .find(|(_, v)| v == &value)
                                    .map(|(l, _)| l.to_string())
                                    .unwrap_or_else(|| value_label(&value)),
                            ),
                            TextFont {
                                font: ui_font.0.clone().into(),
                                font_size: FontSize::Px(14.0),
                                ..default()
                            },
                            TextColor(if state.rebind == Some(def.id) {
                                Color::srgb(1.0, 0.85, 0.4)
                            } else {
                                Color::srgb(0.9, 0.9, 0.9)
                            }),
                        ));
                        // 控件按钮按 kind 生成
                        let buttons: Vec<(&str, SettingsAction)> = match def.kind {
                            SettingKind::Bool => vec![("切换", SettingsAction::SettingToggle { id: def.id })],
                            SettingKind::Int { .. } | SettingKind::Float { .. } => vec![
                                ("-", SettingsAction::SettingDec { id: def.id }),
                                ("+", SettingsAction::SettingInc { id: def.id }),
                            ],
                            SettingKind::Enum => vec![("下一个", SettingsAction::SettingCycle { id: def.id })],
                            SettingKind::KeyCode => vec![("重绑", SettingsAction::SettingRebind { id: def.id })],
                            SettingKind::Text => vec![],
                        };
                        for (label, action) in buttons {
                            parent.spawn((
                                Button,
                                action,
                                Node {
                                    width: Val::Px(56.0),
                                    height: Val::Px(24.0),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    ..default()
                                },
                                BackgroundColor(Color::srgb(0.3, 0.3, 0.45)),
                                children![(
                                    Text::new(label),
                                    TextFont {
                                        font: ui_font.0.clone().into(),
                                        font_size: FontSize::Px(13.0),
                                        ..default()
                                    },
                                    TextColor(Color::WHITE),
                                )],
                            ));
                        }
                    });
            }
        });
        state.rendered_settings = ids;
    }

    // 文件夹列表：与已渲染状态不一致时重建
    let folders = db.list_folders().unwrap_or_default();
    if folders == state.rendered_folders {
        return;
    }
    for row in &folder_rows {
        commands.entity(row).despawn();
    }
    let Ok(list) = list_q.single() else {
        return;
    };
    let rows: Vec<(PathBuf, String)> = folders
        .iter()
        .map(|p| (p.clone(), p.display().to_string()))
        .collect();
    commands.entity(list).with_children(|parent| {
        for (path, display) in rows {
            parent
                .spawn((
                    FolderRow,
                    Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(8.0),
                        align_items: AlignItems::Center,
                        ..default()
                    },
                ))
                .with_children(|parent| {
                    parent.spawn((
                        Text::new(display),
                        TextFont {
                            font: ui_font.0.clone().into(),
                            font_size: FontSize::Px(14.0),
                            ..default()
                        },
                        TextColor(Color::srgb(0.85, 0.85, 0.85)),
                    ));
                    parent.spawn((
                        Button,
                        SettingsAction::RemoveFolder { path },
                        Node {
                            width: Val::Px(48.0),
                            height: Val::Px(24.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        BackgroundColor(Color::srgb(0.5, 0.25, 0.25)),
                        children![(
                            Text::new("移除"),
                            TextFont {
                                font: ui_font.0.clone().into(),
                                font_size: FontSize::Px(13.0),
                                ..default()
                            },
                            TextColor(Color::WHITE),
                        )],
                    ));
                });
        }
    });
    state.rendered_folders = folders;
}
