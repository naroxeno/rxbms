//! 核心模块：应用状态机、全局资源与调试辅助。

pub mod keybind;
pub mod settings;
pub mod state;

use bevy::prelude::*;

use self::state::AppState;

/// 核心插件：注册全局状态机与跨模块资源。
pub struct CorePlugin;

impl Plugin for CorePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(settings::SettingsStorePlugin)
            .init_state::<AppState>()
            .add_systems(Startup, setup_core)
            .add_systems(OnEnter(AppState::Startup), enter_startup)
            .add_systems(
                Update,
                (
                    update_state_label,
                    // 设置界面内数字键留给输入框，调试切换仅在其它状态生效
                    debug_state_switch.run_if(not(in_state(AppState::Settings))),
                ),
            );
    }
}

/// 全局 UI 字体：Noto Sans CJK SC（支持中日文，避免方块字）。
///
/// 所有 UI 文本的 `TextFont.font` 显式引用此句柄（Bevy 0.19 中
/// `FontSource::Handle` 是可靠方式；替换全局默认字体 handle 不会触发
/// parley 字体集重建，不可用）。
#[derive(Resource, Clone)]
pub struct UiFont(pub Handle<Font>);

/// 界面标记：左上角的状态指示文本。
#[derive(Component)]
struct StatusLabel;

/// Startup 阶段入口：铺面库扫描由 [`crate::database`] 负责，
/// 扫描完成后由该插件切换到选曲界面。
fn enter_startup() {
    info!("[core] 进入 Startup，等待铺面库扫描…");
}

fn setup_core(mut commands: Commands, asset_server: Res<AssetServer>) {
    let ui_font = asset_server.load("fonts/NotoSansCJK-SC-Regular.otf");
    commands.insert_resource(UiFont(ui_font.clone()));
    commands.spawn(Camera2d);
    commands.spawn((
        StatusLabel,
        Text::new("state: Startup"),
        TextFont {
            font: ui_font.into(),
            font_size: FontSize::Px(24.0),
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(8.0),
            ..default()
        },
    ));
}

fn update_state_label(
    state: Res<State<AppState>>,
    mut labels: Query<&mut Text, With<StatusLabel>>,
) {
    let label = format!("state: {}  (按 1-5 切换调试)", state.get().label());
    for mut text in &mut labels {
        text.0 = label.clone();
    }
}

/// 调试辅助：数字键 1-5 直接切换到对应状态。
fn debug_state_switch(
    keys: Res<ButtonInput<KeyCode>>,
    mut next: ResMut<NextState<AppState>>,
) {
    let target = if keys.just_pressed(KeyCode::Digit1) {
        Some(AppState::SongSelect)
    } else if keys.just_pressed(KeyCode::Digit2) {
        Some(AppState::Gameplay)
    } else if keys.just_pressed(KeyCode::Digit3) {
        Some(AppState::Result)
    } else if keys.just_pressed(KeyCode::Digit4) {
        Some(AppState::Settings)
    } else if keys.just_pressed(KeyCode::Digit5) {
        Some(AppState::Startup)
    } else {
        None
    };
    if let Some(target) = target {
        NextState::set_if_neq(&mut next, target);
    }
}
