//! 键位绑定：轨道（转盘 + 键 1-7）→ 键盘按键。
//!
//! 默认布局（BMS 7k 常用）：盘 = A，键 1-7 = S D F 空格 J K L。
//! 5k 谱面复用同一套绑定（只用到键 1-5 + 盘）。
//!
//! 值由统一设置系统（`core::settings::SettingsStore`）提供，
//! 通过 [`KeyBindings::from_store`] 派生；持久化由设置系统负责。

use std::collections::HashMap;

use bms_rs::chart::prelude::Key;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::settings::SettingsStore;

/// 可绑定的轨道目标（BMS 键位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BindTarget {
    /// 转盘（scratch）。
    Scratch,
    /// 键 1-7。
    Key(u8),
}

/// 游玩模式（不同模式的键位独立配置）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlayMode {
    /// 5 键 + 盘。
    FiveKey,
    /// 7 键 + 盘。
    SevenKey,
}

impl PlayMode {
    /// 全部支持的游玩模式。
    pub const ALL: [PlayMode; 2] = [PlayMode::FiveKey, PlayMode::SevenKey];

    /// 显示名（设置界面分组）。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::FiveKey => "5K",
            Self::SevenKey => "7K",
        }
    }

    /// 设置项 id 前缀（单一来源）。
    #[must_use]
    pub const fn setting_prefix(self) -> &'static str {
        match self {
            Self::FiveKey => "key5",
            Self::SevenKey => "key7",
        }
    }

    /// 该模式使用的最大键号。
    #[must_use]
    pub const fn max_key(self) -> u8 {
        match self {
            Self::FiveKey => 5,
            Self::SevenKey => 7,
        }
    }
}

impl BindTarget {
    /// 显示名（设置界面）。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Scratch => "盘",
            Self::Key(n) => match n {
                1 => "键1",
                2 => "键2",
                3 => "键3",
                4 => "键4",
                5 => "键5",
                6 => "键6",
                7 => "键7",
                _ => "?",
            },
        }
    }

    /// 对应设置项的 id（带模式前缀，单一来源）。
    #[must_use]
    pub fn setting_id(self, mode: PlayMode) -> String {
        let base = match self {
            Self::Scratch => "scratch".to_string(),
            Self::Key(n) => n.to_string(),
        };
        format!("{}_{}", mode.setting_prefix(), base)
    }
}

impl From<BindTarget> for Key {
    fn from(value: BindTarget) -> Self {
        match value {
            BindTarget::Scratch => Key::Scratch(1),
            BindTarget::Key(n) => Key::Key(n),
        }
    }
}

/// 键位绑定映射（轨道 → 按键）。
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct KeyBindings {
    map: HashMap<BindTarget, KeyCode>,
}

/// 各轨道默认按键（单一来源：设置系统注册表与键位派生共用）。
impl KeyBindings {
    /// 默认键位表（盘 = A，键 = S D F Space J K L）。
    #[must_use]
    pub fn default_map() -> HashMap<BindTarget, KeyCode> {
        [
            (BindTarget::Scratch, KeyCode::KeyA),
            (BindTarget::Key(1), KeyCode::KeyS),
            (BindTarget::Key(2), KeyCode::KeyD),
            (BindTarget::Key(3), KeyCode::KeyF),
            (BindTarget::Key(4), KeyCode::Space),
            (BindTarget::Key(5), KeyCode::KeyJ),
            (BindTarget::Key(6), KeyCode::KeyK),
            (BindTarget::Key(7), KeyCode::KeyL),
        ]
        .into_iter()
        .collect()
    }

    /// 从统一设置表按模式派生（设置项 id 带模式前缀，如 `key7_1`）。
    #[must_use]
    pub fn from_store(store: &SettingsStore, mode: PlayMode) -> Self {
        let map = Self::default_map()
            .into_iter()
            .filter(|(t, _)| match t {
                BindTarget::Scratch => true,
                BindTarget::Key(n) => *n <= mode.max_key(),
            })
            .map(|(t, default)| {
                let code = store.get_keycode(&t.setting_id(mode), default);
                (t, code)
            })
            .collect();
        Self { map }
    }

    /// 全部绑定（按 盘 → 键 1-7 排序，供设置界面展示）。
    #[allow(dead_code)] // 供测试使用
    #[must_use]
    pub fn entries(&self) -> Vec<(BindTarget, KeyCode)> {
        let mut entries: Vec<_> = self.map.iter().map(|(t, c)| (*t, *c)).collect();
        entries.sort_by_key(|(t, _)| match t {
            BindTarget::Scratch => (0u8, 0u8),
            BindTarget::Key(n) => (1, *n),
        });
        entries
    }

    /// 目标 → 按键。
    #[allow(dead_code)] // 供测试使用
    #[must_use]
    pub fn get(&self, target: BindTarget) -> Option<KeyCode> {
        self.map.get(&target).copied()
    }

    /// 绑定（同一按键在其他目标上的旧绑定会被移除，避免冲突）。
    #[allow(dead_code)] // 供测试使用
    pub fn set(&mut self, target: BindTarget, code: KeyCode) {
        self.map.retain(|t, c| *t == target || *c != code);
        self.map.insert(target, code);
    }

    /// 按键 → 目标（输入判定用反向查找）。
    #[must_use]
    pub fn target_for(&self, code: KeyCode) -> Option<BindTarget> {
        self.map
            .iter()
            .find(|(_, c)| **c == code)
            .map(|(t, _)| *t)
    }
}

/// 各游玩模式的键位绑定集合（Resource，游玩时按谱面模式取用）。
#[derive(Resource, Debug, Clone)]
pub struct KeyBindingsByMode {
    five: KeyBindings,
    seven: KeyBindings,
}

impl KeyBindingsByMode {
    /// 从统一设置表按全部模式派生。
    #[must_use]
    pub fn from_store(store: &SettingsStore) -> Self {
        Self {
            five: KeyBindings::from_store(store, PlayMode::FiveKey),
            seven: KeyBindings::from_store(store, PlayMode::SevenKey),
        }
    }

    /// 取指定模式的绑定。
    #[must_use]
    pub fn for_mode(&self, mode: PlayMode) -> &KeyBindings {
        match mode {
            PlayMode::FiveKey => &self.five,
            PlayMode::SevenKey => &self.seven,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_store_respects_mode() {
        let registry = crate::core::settings::SettingsRegistry::builtin();
        let store = crate::core::settings::SettingsStore::from_registry(&registry, None);
        let five = KeyBindings::from_store(&store, PlayMode::FiveKey);
        let seven = KeyBindings::from_store(&store, PlayMode::SevenKey);
        // 5K：盘 + 键1-5；7K：盘 + 键1-7
        assert!(five.get(BindTarget::Key(5)).is_some());
        assert!(five.get(BindTarget::Key(6)).is_none());
        assert!(five.get(BindTarget::Scratch).is_some());
        assert!(seven.get(BindTarget::Key(6)).is_some());
        assert!(seven.get(BindTarget::Key(7)).is_some());
    }

    #[test]
    fn default_bindings_complete() {
        let b = KeyBindings {
            map: [
                (BindTarget::Scratch, KeyCode::KeyA),
                (BindTarget::Key(1), KeyCode::KeyS),
                (BindTarget::Key(2), KeyCode::KeyD),
                (BindTarget::Key(3), KeyCode::KeyF),
                (BindTarget::Key(4), KeyCode::Space),
                (BindTarget::Key(5), KeyCode::KeyJ),
                (BindTarget::Key(6), KeyCode::KeyK),
                (BindTarget::Key(7), KeyCode::KeyL),
            ]
            .into_iter()
            .collect(),
        };
        assert_eq!(b.entries().len(), 8);
        assert_eq!(b.target_for(KeyCode::KeyA), Some(BindTarget::Scratch));
        assert_eq!(b.target_for(KeyCode::KeyS), Some(BindTarget::Key(1)));
    }

    #[test]
    fn set_removes_conflict() {
        let mut b = KeyBindings {
            map: [
                (BindTarget::Key(1), KeyCode::KeyS),
                (BindTarget::Key(7), KeyCode::KeyL),
            ]
            .into_iter()
            .collect(),
        };
        b.set(BindTarget::Key(7), KeyCode::KeyS);
        assert_eq!(b.get(BindTarget::Key(1)), None);
        assert_eq!(b.get(BindTarget::Key(7)), Some(KeyCode::KeyS));
    }
}
