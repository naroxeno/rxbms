//! rxbms — 基于 Bevy 的 BMS 铺面播放器。

mod audio;
mod core;
mod database;
mod gameplay;
mod record;
mod result;
mod select;
mod settings;
mod skin;
use bevy::{
    asset::{AssetPlugin, UnapprovedPathMode, io::{AssetSourceBuilder, file::FileAssetReader}},
    audio::AudioPlugin,
    log::{DEFAULT_FILTER, LogPlugin},
    prelude::*,
};

use crate::{
    audio::AudioManagerPlugin,
    core::CorePlugin,
    database::SongDatabasePlugin,
    gameplay::GameplayPlugin,
    record::RecordPlugin,
    result::ResultPlugin,
    select::SongSelectPlugin,
    settings::SettingsPlugin,
    skin::SkinPlugin,
};

fn main() {
    App::new()
        // 注册文件系统根 source：BMS 音频在铺面目录（任意系统路径），
        // 以 `fs:///abs/path` 形式加载（必须在 AssetPlugin 之前注册）
        .register_asset_source(
            "fs",
            AssetSourceBuilder::new(|| Box::new(FileAssetReader::new("/"))),
        )
        .add_plugins(
            DefaultPlugins
                .set(AssetPlugin {
                    // BMS 音频在铺面目录（系统任意路径），放行未批准的绝对路径加载
                    unapproved_path_mode: UnapprovedPathMode::Allow,
                    ..default()
                })
                .set(LogPlugin {
                    // 压制 icu_provider 的日语分词模型噪音（Bevy 内部已知问题，见 Cargo.toml 注释）
                    filter: format!("{DEFAULT_FILTER},icu_provider=error"),
                    ..default()
                })
                // 音频自行管理（kira 驱动，见 audio.rs；禁用 Bevy 内置避免双输出流冲突）
                .disable::<AudioPlugin>(),
        )
        .add_plugins(CorePlugin)
        .add_plugins(SkinPlugin)
        .add_plugins(AudioManagerPlugin)
        .add_plugins(SongDatabasePlugin)
        .add_plugins(SongSelectPlugin)
        .add_plugins(GameplayPlugin)
        .add_plugins(ResultPlugin)
        .add_plugins(RecordPlugin)
        .add_plugins(SettingsPlugin)
        .run();
}
