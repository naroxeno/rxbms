//! 轨道游玩状态：每个 Lane 独立维护（为后续打击特效等准备）。
//!
//! 三种长音（参考 beatoraja，由 `#LNTYPE` 决定）：
//! - **LN**（普通长音）：头部判定；窗口内松手 → 尾部判定（Good）；按住到尾 → 无判定；
//! - **CN**（充电音符）：同 LN 结构，但松手尾判更严（待尾判等级 = 松手时判定）；
//! - **HCN**（地狱充电）：持有期间持续回血，松手断 → 判尾扣血。

use std::collections::HashMap;

use bevy::prelude::*;
use bms_rs::{bms::command::LnMode, chart::prelude::Key};

use super::judge::{JudgeWindows, Judgement};

/// 长音种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LnKind {
    /// 普通长音（无尾判语义，按住完成）。
    #[default]
    LongNote,
    /// 充电音符（带尾判）。
    ChargeNote,
    /// 地狱充电音符（带尾判 + 持有回血）。
    HellChargeNote,
}

impl From<LnMode> for LnKind {
    fn from(mode: LnMode) -> Self {
        match mode {
            LnMode::Ln => Self::LongNote,
            LnMode::Cn => Self::ChargeNote,
            LnMode::Hcn => Self::HellChargeNote,
        }
    }
}

impl LnKind {
    /// 显示名（Lua 皮肤接管后暂无调用，保留）。
    #[must_use]
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            Self::LongNote => "LN",
            Self::ChargeNote => "CN",
            Self::HellChargeNote => "HCN",
        }
    }
}

/// 单个轨道上活跃长音的状态机（beatoraja LaneState 对应）。
#[derive(Debug, Default)]
pub struct LnState {
    /// 激活的 LN 音符索引（`None` = 无活跃 LN）。
    pub processing: Option<usize>,
    /// 长音种类。
    pub kind: LnKind,
    /// 头部判定等级。
    pub lnstart_judge: Option<Judgement>,
    /// 头部判定时间差（秒）。
    pub lnstart_duration: f64,
    /// 中途松手时刻（`None` = 保持中）。
    pub release_time: Option<f64>,
    /// 待尾部判定的等级（中途松手过才有）。
    pub lnend_judge: Option<Judgement>,
    /// HCN 上次回血时刻。
    pub last_heal: f64,
}

/// 单个轨道的完整游玩状态（独立维护，为打击特效预留）。
#[derive(Debug, Default)]
pub struct LaneState {
    /// 活跃 LN 状态机。
    pub ln: LnState,
    /// 最近一次命中判定（打击特效 / 轨道反馈预留）。
    pub last_hit: Option<(Judgement, f64)>,
}

/// 全部轨道的状态（Resource，Key → LaneState）。
#[derive(Resource, Default)]
pub struct LaneStates {
    lanes: HashMap<Key, LaneState>,
}

impl LaneStates {
    /// 获取（或创建）某轨道的状态。
    pub fn lane(&mut self, key: Key) -> &mut LaneState {
        self.lanes.entry(key).or_default()
    }

    /// 迭代所有轨道的状态（游戏逻辑读取用）。
    pub fn iter(&self) -> impl Iterator<Item = (&Key, &LaneState)> {
        self.lanes.iter()
    }
    /// 迭代所有轨道的状态（游戏逻辑读取用）。
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&Key, &mut LaneState)> {
        self.lanes.iter_mut()
    }

    /// 任意轨道是否持有指定音符（LN head 已命中）。
    #[must_use]
    pub fn holds_note(&self, idx: usize) -> bool {
        self.lanes.values().any(|lane| lane.ln.processing == Some(idx))
    }
}

/// LN 头部命中：激活 LN（按下且命中时调用）。
pub fn start_ln(
    state: &mut LnState,
    idx: usize,
    kind: LnKind,
    judge: Judgement,
    duration: f64,
) {
    state.processing = Some(idx);
    state.kind = kind;
    state.lnstart_judge = Some(judge);
    state.lnstart_duration = duration;
    state.release_time = None;
    state.lnend_judge = None;
}

/// LN 松手（按键释放时调用）。
///
/// - 在 Good 窗口内且尾部未到 → 待尾判（返回 `None`，LN 继续，不断连）；
/// - 否则（早松超窗）→ 立即判尾（返回 `Some(Pr)`）。
pub fn release_ln(
    state: &mut LnState,
    now: f64,
    tail_sec: f64,
    windows: &JudgeWindows,
) -> Option<Judgement> {
    let dmtime = tail_sec - now; // 释放时刻距尾部的时间（正 = 尾部未到）
    if dmtime > 0.0 && dmtime * 1000.0 <= windows.gd_ms {
        state.release_time = Some(now);
        state.lnend_judge = Some(Judgement::Gd);
        None
    } else {
        Some(Judgement::Pr)
    }
}

/// LN 每帧推进（尾部到达检测）。
///
/// 返回：
/// - `None`：尾部未到达，继续；
/// - `Some(Some(j))`：尾部到达且有待尾判 → 判定 `j`；
/// - `Some(None)`：尾部到达且按住完成 → 无判定。
pub fn update_ln(
    state: &LnState,
    head_y: f64,
    length_y: f64,
    now_y: f64,
) -> Option<Option<Judgement>> {
    let tail_y = head_y + length_y;
    if now_y < tail_y {
        return None;
    }
    if let Some(j) = state.lnend_judge {
        Some(Some(j))
    } else {
        Some(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn windows() -> JudgeWindows {
        JudgeWindows::default()
    }

    #[test]
    fn release_within_window_defers() {
        let mut st = LnState::default();
        start_ln(&mut st, 0, LnKind::LongNote, Judgement::Pg, 0.0);
        // 尾部 1s 后，尾部前 50ms 释放（Good 窗口内）
        let r = release_ln(&mut st, 0.95, 1.0, &windows());
        assert!(r.is_none(), "窗口内松手应待尾判");
        assert!(st.release_time.is_some());
        assert_eq!(st.lnend_judge, Some(Judgement::Gd));
    }

    #[test]
    fn release_early_misses() {
        let mut st = LnState::default();
        start_ln(&mut st, 0, LnKind::LongNote, Judgement::Pg, 0.0);
        // 尾部 0.5s 后释放 → 超出 Good 窗口（早松超窗）
        let r = release_ln(&mut st, 0.0, 0.5, &windows());
        assert_eq!(r, Some(Judgement::Pr), "早松应立即判尾");
    }

    #[test]
    fn tail_reached_held_completes() {
        let mut st = LnState::default();
        start_ln(&mut st, 0, LnKind::ChargeNote, Judgement::Pg, 0.0);
        // 按住到尾：无待尾判
        assert_eq!(update_ln(&st, 0.0, 10.0, 11.0), Some(None));
    }

    #[test]
    fn tail_reached_after_release_judges() {
        let mut st = LnState::default();
        start_ln(&mut st, 0, LnKind::ChargeNote, Judgement::Pg, 0.0);
        // 尾部前 50ms 松手 → 待尾判
        assert!(release_ln(&mut st, 4.95, 5.0, &windows()).is_none());
        // 尾部到达 → 判 Good
        assert_eq!(update_ln(&st, 0.0, 10.0, 11.0), Some(Some(Judgement::Gd)));
    }

    #[test]
    fn not_reached_yet() {
        let st = LnState::default();
        assert_eq!(update_ln(&st, 0.0, 10.0, 5.0), None);
    }

    #[test]
    fn lntype_kinds() {
        assert_eq!(LnKind::from(LnMode::Ln), LnKind::LongNote);
        assert_eq!(LnKind::from(LnMode::Cn), LnKind::ChargeNote);
        assert_eq!(LnKind::from(LnMode::Hcn), LnKind::HellChargeNote);
    }
}
