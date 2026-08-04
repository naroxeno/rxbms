//! 统一设置系统：一张注册表（大表）管理所有内建设置项，支持可拓展。
//!
//! 设计：
//! - [`SettingsRegistry`]：静态定义表（id / 名称 / 分类 / 类型 / 默认值 / 枚举选项），
//!   新增设置项只需在 `builtin()` 加一行；
//! - [`SettingsStore`]：运行时值存储（`HashMap<String, SettingValue>`），
//!   设置界面按注册表动态渲染，改值即存；
//! - 持久化到 `~/.rxbms/config.json`（`settings` 字段），启动加载、变更保存。
//!
//! 后续皮肤切换等新设置只需注册新定义 + 消费方读取。

use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::keybind::KeyBindingsByMode;

// ---------- 设置值 ----------

/// 设置项的值类型。
#[derive(Debug, Clone, PartialEq)]
pub enum SettingValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    KeyCode(KeyCode),
}

// 手写 serde：JSON 值 → 简单标量表示。
// 注意：`SettingValue::KeyCode` 序列化为 KeyCode 的 serde 形式（bevy serialize feature，
// unit variant 即变体名字符串），反序列化时按 untagged 顺序先试 KeyCode 再回退其他标量。
impl Serialize for SettingValue {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Bool(b) => b.serialize(s),
            Self::Int(i) => i.serialize(s),
            Self::Float(f) => f.serialize(s),
            Self::String(st) => st.serialize(s),
            Self::KeyCode(k) => k.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for SettingValue {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Bool(bool),
            Int(i64),
            Float(f64),
            Key(KeyCode),
            Str(String),
        }
        // 顺序：Key 在 Str 之前（KeyCode 变体名可回退为普通字符串），
        // Int/Float 在前避免 KeyCode 数字形式歧义（实际 KeyCode 序列化为字符串）。
        match Raw::deserialize(d)? {
            Raw::Bool(b) => Ok(Self::Bool(b)),
            Raw::Int(i) => Ok(Self::Int(i)),
            Raw::Float(f) => Ok(Self::Float(f)),
            Raw::Key(k) => Ok(Self::KeyCode(k)),
            Raw::Str(st) => Ok(Self::String(st)),
        }
    }
}

// ---------- 设置定义 ----------

/// 设置分类（设置界面按此分组）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingCategory {
    /// 玩法（判定等）。
    Gameplay,
    /// 显示（下落速度等）。
    Display,
    /// 音频（音量等）。
    Audio,
    /// 键位。
    Keys,
    /// 皮肤。
    Skin,
}

impl SettingCategory {
    /// 分类显示名。
    pub fn label(self) -> &'static str {
        match self {
            Self::Gameplay => "玩法",
            Self::Display => "显示",
            Self::Audio => "音频",
            Self::Keys => "键位",
            Self::Skin => "皮肤",
        }
    }
}

/// 设置控件类型。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingKind {
    /// 开/关。
    Bool,
    /// 数值（步进调节）。
    Int { min: i64, max: i64, step: i64 },
    Float { min: f64, max: f64, step: f64 },
    /// 枚举（循环切换）。
    Enum,
    /// 文本（如皮肤路径）。
    Text,
    /// 按键（点击重绑）。
    KeyCode,
}

/// 单个设置项定义。
#[derive(Debug, Clone)]
pub struct SettingDef {
    /// 唯一标识（`key_1`、`volume` 等）。
    pub id: &'static str,
    /// 显示名。
    pub name: &'static str,
    /// 分类。
    pub category: SettingCategory,
    /// 控件类型。
    pub kind: SettingKind,
    /// 默认值。
    pub default: SettingValue,
    /// 枚举选项（kind=Enum 时使用）：(显示名, 值)。
    pub options: Vec<(&'static str, SettingValue)>,
}

impl SettingDef {
    /// 布尔设置。
    pub fn bool_(id: &'static str, name: &'static str, category: SettingCategory, default: bool) -> Self {
        Self { id, name, category, kind: SettingKind::Bool, default: SettingValue::Bool(default), options: vec![] }
    }

    /// 整数设置（预留：目前无整数设置项）。
    #[allow(dead_code)]
    pub fn int_(id: &'static str, name: &'static str, category: SettingCategory, min: i64, max: i64, step: i64, default: i64) -> Self {
        Self { id, name, category, kind: SettingKind::Int { min, max, step }, default: SettingValue::Int(default), options: vec![] }
    }

    /// 浮点设置。
    pub fn float_(id: &'static str, name: &'static str, category: SettingCategory, min: f64, max: f64, step: f64, default: f64) -> Self {
        Self { id, name, category, kind: SettingKind::Float { min, max, step }, default: SettingValue::Float(default), options: vec![] }
    }

    /// 枚举设置。
    pub fn enum_(id: &'static str, name: &'static str, category: SettingCategory, options: Vec<(&'static str, SettingValue)>, default: SettingValue) -> Self {
        Self { id, name, category, kind: SettingKind::Enum, default, options }
    }

    /// 文本设置。
    pub fn text_(id: &'static str, name: &'static str, category: SettingCategory, default: &str) -> Self {
        Self { id, name, category, kind: SettingKind::Text, default: SettingValue::String(default.to_string()), options: vec![] }
    }

    /// 键位设置。
    pub fn keycode_(id: &'static str, name: &'static str, default: KeyCode) -> Self {
        Self { id, name, category: SettingCategory::Keys, kind: SettingKind::KeyCode, default: SettingValue::KeyCode(default), options: vec![] }
    }
}

// ---------- 注册表 ----------

/// 内建设置项注册表（大表）。
#[derive(Resource)]
pub struct SettingsRegistry {
    pub defs: Vec<SettingDef>,
}

impl SettingsRegistry {
    /// 所有内建设置项定义（新增设置项在此加一行即可）。
    pub fn builtin() -> Self {
        let mut defs = vec![
            // 玩法
            SettingDef::enum_(
                "judge_level", "判定难度", SettingCategory::Gameplay,
                vec![
                    ("VeryHard", SettingValue::String("VeryHard".into())),
                    ("Hard", SettingValue::String("Hard".into())),
                    ("Normal", SettingValue::String("Normal".into())),
                    ("Easy", SettingValue::String("Easy".into())),
                ],
                SettingValue::String("Normal".into()),
            ),
            // 血条类型（beatoraja GrooveGauge 索引 0-8）
            SettingDef::enum_(
                "gauge_type", "血条类型", SettingCategory::Gameplay,
                vec![
                    ("Assist Easy", SettingValue::Int(0)),
                    ("Easy", SettingValue::Int(1)),
                    ("Normal", SettingValue::Int(2)),
                    ("Hard", SettingValue::Int(3)),
                    ("ExHard", SettingValue::Int(4)),
                    ("Hazard", SettingValue::Int(5)),
                    ("Class", SettingValue::Int(6)),
                    ("ExClass", SettingValue::Int(7)),
                    ("ExHardClass", SettingValue::Int(8)),
                ],
                SettingValue::Int(2),
            ),
            // 显示
            SettingDef::bool_("show_fast_slow", "Fast/Slow 显示", SettingCategory::Display, false),
            SettingDef::float_("scroll_speed", "下落速度", SettingCategory::Display, 0.5, 3.0, 0.1, 1.0),
            // 垂直同步（运行时切换窗口 PresentMode；关闭则不锁帧）
            SettingDef::bool_("vsync", "垂直同步 (vsync)", SettingCategory::Display, true),
            // 音频
            SettingDef::float_("volume", "全局音量", SettingCategory::Audio, 0.0, 1.0, 0.05, 1.0),
            // 皮肤
            SettingDef::text_("skin_path", "皮肤路径", SettingCategory::Skin, "test_skin/Play"),
        ];
        // 键位：按游玩模式分组（5K：盘 + 键1-5；7K：盘 + 键1-7）
        // 默认值单一来源 `KeyBindings::default_map`
        for mode in crate::core::keybind::PlayMode::ALL {
            for (target, default) in crate::core::keybind::KeyBindings::default_map() {
                let is_scratch = matches!(target, crate::core::keybind::BindTarget::Scratch);
                let in_range = matches!(target, crate::core::keybind::BindTarget::Key(n) if n <= mode.max_key());
                if !is_scratch && !in_range {
                    continue;
                }
                defs.push(SettingDef::keycode_(
                    Box::leak(target.setting_id(mode).into_boxed_str()),
                    Box::leak(format!("{} {}", mode.label(), target.label()).into_boxed_str()),
                    default,
                ));
            }
        }
        Self { defs }
    }

    /// 按 id 查定义。
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&SettingDef> {
        self.defs.iter().find(|d| d.id == id)
    }
}

// ---------- 运行时存储 ----------

/// 设置值存储（Resource）：启动从 config.json 加载，修改即保存。
#[derive(Resource, Debug, Clone, Default)]
pub struct SettingsStore {
    values: HashMap<String, SettingValue>,
}

impl SettingsStore {
    /// 由注册表构建：填充默认值，再用已保存值覆盖。
    pub fn from_registry(registry: &SettingsRegistry, saved: Option<&HashMap<String, SettingValue>>) -> Self {
        let mut values = HashMap::new();
        for def in &registry.defs {
            values.insert(def.id.to_string(), def.default.clone());
        }
        if let Some(saved) = saved {
            for (k, v) in saved {
                if values.contains_key(k) {
                    values.insert(k.clone(), v.clone());
                }
            }
        }
        Self { values }
    }

    /// 读取布尔。
    #[must_use]
    pub fn get_bool(&self, id: &str, default: bool) -> bool {
        match self.values.get(id) {
            Some(SettingValue::Bool(b)) => *b,
            _ => default,
        }
    }

    /// 读取整数。
    #[must_use]
    pub fn get_int(&self, id: &str, default: i64) -> i64 {
        match self.values.get(id) {
            Some(SettingValue::Int(i)) => *i,
            _ => default,
        }
    }

    /// 读取浮点。
    #[must_use]
    pub fn get_float(&self, id: &str, default: f64) -> f64 {
        match self.values.get(id) {
            Some(SettingValue::Float(f)) => *f,
            _ => default,
        }
    }

    /// 读取字符串。
    #[must_use]
    pub fn get_string(&self, id: &str, default: &str) -> String {
        match self.values.get(id) {
            Some(SettingValue::String(s)) => s.clone(),
            _ => default.to_string(),
        }
    }

    /// 读取按键。
    #[must_use]
    pub fn get_keycode(&self, id: &str, default: KeyCode) -> KeyCode {
        match self.values.get(id) {
            Some(SettingValue::KeyCode(k)) => *k,
            _ => default,
        }
    }

    /// 写入并标记变更（由消费系统触发保存）。
    pub fn set(&mut self, id: &str, value: SettingValue) {
        self.values.insert(id.to_string(), value);
    }

    /// 设置项当前值（供 UI 渲染）。
    #[must_use]
    pub fn value(&self, id: &str) -> Option<&SettingValue> {
        self.values.get(id)
    }

    /// 全部键值（持久化用）。
    pub fn all(&self) -> &HashMap<String, SettingValue> {
        &self.values
    }
}

// ---------- 持久化 ----------

/// 配置文件结构。
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SettingsFile {
    /// 设置值表。
    pub settings: Option<HashMap<String, SettingValue>>,
}

/// 配置文件路径（`~/.rxbms/config.json`）。
#[must_use]
pub fn config_path() -> PathBuf {
    crate::database::data_dir().join("config.json")
}

/// 加载配置文件。
#[must_use]
pub fn load_settings_file() -> SettingsFile {
    match fs::read_to_string(config_path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            warn!("[settings] 解析 {} 失败，使用默认: {e}", config_path().display());
            SettingsFile::default()
        }),
        Err(_) => SettingsFile::default(),
    }
}

/// 保存设置文件。
///
/// # Errors
///
/// 目录创建、序列化或写入失败时返回错误。
pub fn save_settings_file(file: &SettingsFile) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建目录失败 {}: {e}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(file).map_err(|e| format!("序列化失败: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("写入失败 {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_value_keycode_roundtrip() {
        // 覆盖非候选键（此前 parse_keycode 只识别少量键，会丢失绑定）
        for code in [
            KeyCode::F1,
            KeyCode::Digit7,
            KeyCode::Numpad4,
            KeyCode::ArrowLeft,
            KeyCode::ShiftLeft,
            KeyCode::KeyA,
            KeyCode::Space,
        ] {
            let v = SettingValue::KeyCode(code);
            let json = serde_json::to_string(&v).expect("序列化");
            let back: SettingValue = serde_json::from_str(&json).expect("反序列化");
            assert_eq!(back, SettingValue::KeyCode(code), "键位往返丢失: {code:?} -> {json}");
        }
    }

    #[test]
    fn setting_value_scalar_roundtrip() {
        let cases = [
            SettingValue::Bool(true),
            SettingValue::Int(42),
            SettingValue::Float(0.75),
            SettingValue::String("test_skin/Play".into()),
        ];
        for v in cases {
            let json = serde_json::to_string(&v).expect("序列化");
            let back: SettingValue = serde_json::from_str(&json).expect("反序列化");
            assert_eq!(back, v, "标量往返失败: {json}");
        }
    }
}

/// 设置存储插件：初始化注册表 + store，加载 config.json。
pub struct SettingsStorePlugin;

impl Plugin for SettingsStorePlugin {
    fn build(&self, app: &mut App) {
        let registry = SettingsRegistry::builtin();
        let file = load_settings_file();
        let saved = file.settings.unwrap_or_default();
        let store = SettingsStore::from_registry(&registry, Some(&saved));
        let bindings = KeyBindingsByMode::from_store(&store);
        app.insert_resource(registry)
            .insert_resource(store)
            .insert_resource(bindings);
    }
}
