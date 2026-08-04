//! 判定系统：输入命中分级、计数、连击与 EX 分数。
//!
//! 判定窗口采用 **Lunatic Rave 2** 默认（`#RANK` → 判定难度）：
//!
//! | 难度 | PGREAT | GREAT | GOOD | BAD | 空POOR(过早) |
//! |---|---|---|---|---|---|
//! | Easy    | ±21ms | ±60ms  | ±120ms | ±200ms | -1000ms |
//! | Normal  | ±18ms | ±40ms  | ±100ms | ±200ms | -1000ms |
//! | Hard    | ±15ms | ±30ms  | ±60ms  | ±200ms | -1000ms |
//! | VeryHard| ±8ms  | ±24ms  | ±40ms  | ±200ms | -100ms  |
//!
//! 超出 BAD 窗口的早按 = 空 POOR（记录但不打断连击、不消费音符）；
//! 音符经过后未命中 = 普通 POOR（打断连击）。
//! EX 分数 = 2×PGREAT + GREAT。

use bms_rs::bms::command::JudgeLevel;
use bevy::prelude::*;

/// 判定等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Judgement {
    /// Perfect Great
    Pg,
    /// Great
    Gr,
    /// Good
    Gd,
    /// Bad（打断连击）
    Bd,
    /// Poor（音符经过未命中，打断连击）
    Pr,
    /// 空 POOR（过早按键 / 多余按键；不断连、不消费音符）
    AirPoor,
}

impl Judgement {
    /// 显示名（Lua 皮肤弹字接管后暂无调用，保留）。
    #[must_use]
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            Self::Pg => "PG",
            Self::Gr => "GR",
            Self::Gd => "GD",
            Self::Bd => "BD",
            Self::Pr => "PR",
            Self::AirPoor => "POOR",
        }
    }
}

/// LR2 判定窗口（毫秒）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JudgeWindows {
    pub pg_ms: f64,
    pub gr_ms: f64,
    pub gd_ms: f64,
    pub bd_ms: f64,
    /// 早按空 POOR 窗口（毫秒）：早于 `bd_ms` 且不早于此值 → 空 POOR。
    pub early_poor_ms: f64,
}

impl Default for JudgeWindows {
    fn default() -> Self {
        Self::for_level(JudgeLevel::Normal)
    }
}

impl JudgeWindows {
    /// `#RANK` 对应的判定难度窗口（LR2 表）。
    #[must_use]
    pub fn for_level(level: JudgeLevel) -> Self {
        match level {
            JudgeLevel::VeryHard => Self {
                pg_ms: 8.0,
                gr_ms: 24.0,
                gd_ms: 40.0,
                bd_ms: 200.0,
                early_poor_ms: 100.0,
            },
            JudgeLevel::Hard => Self {
                pg_ms: 15.0,
                gr_ms: 30.0,
                gd_ms: 60.0,
                bd_ms: 200.0,
                early_poor_ms: 1000.0,
            },
            JudgeLevel::Normal => Self {
                pg_ms: 18.0,
                gr_ms: 40.0,
                gd_ms: 100.0,
                bd_ms: 200.0,
                early_poor_ms: 1000.0,
            },
            JudgeLevel::Easy => Self {
                pg_ms: 21.0,
                gr_ms: 60.0,
                gd_ms: 120.0,
                bd_ms: 200.0,
                early_poor_ms: 1000.0,
            },
            JudgeLevel::OtherInt(_) => Self::for_level(JudgeLevel::Normal),
        }
    }
}

/// 按时间差（秒，`now - note_time`）给出判定。
///
/// 返回 `None` 表示完全忽略（早于空 POOR 窗口或晚于 BAD 窗口之外的早按，
/// 无任何记录）。
#[must_use]
pub fn judge(delta_sec: f64, w: &JudgeWindows) -> Option<Judgement> {
    let ms = delta_sec * 1000.0;
    if (-w.bd_ms..=w.bd_ms).contains(&ms) {
        let a = ms.abs();
        let j = if a <= w.pg_ms {
            Judgement::Pg
        } else if a <= w.gr_ms {
            Judgement::Gr
        } else if a <= w.gd_ms {
            Judgement::Gd
        } else {
            Judgement::Bd
        };
        Some(j)
    } else if ms < -w.bd_ms && ms >= -w.early_poor_ms {
        // 早按空 POOR
        Some(Judgement::AirPoor)
    } else {
        // 太早（早于空 POOR 窗口）→ 忽略；晚于 BAD 窗口由 miss 检测处理
        None
    }
}

/// 判定早/晚方向（Fast/Slow 统计用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JudgeDir {
    /// 无方向（空 POOR / 自动判定 / LN 尾判等）。
    Neutral,
    /// 早判定（Fast）。
    Early,
    /// 晚判定（Slow）。
    Late,
}

/// 游玩过程中的判定统计。
#[derive(Resource, Default, Debug, Clone)]
pub struct JudgeState {
    pub pg: u32,
    pub gr: u32,
    pub gd: u32,
    pub bd: u32,
    pub pr: u32,
    /// 空 POOR 计数（不打断连击）。
    pub air_poor: u32,
    /// 早判定计数（Fast）。
    pub early: u32,
    /// 晚判定计数（Slow）。
    pub late: u32,
    /// 断连次数（BD/PR 每次 +1）。
    pub combo_break: u32,
    /// 当前连击。
    pub combo: u32,
    /// 最大连击。
    pub max_combo: u32,
    /// EX 分数（2×PG + GR）。
    pub ex_score: u32,
}

impl JudgeState {
    /// 记录一次判定，更新计数 / 连击 / EX 分数 / Fast-Slow 统计。
    pub fn record(&mut self, j: Judgement, dir: JudgeDir) {
        match j {
            Judgement::Pg => {
                self.pg += 1;
                self.combo += 1;
                self.ex_score += 2;
            }
            Judgement::Gr => {
                self.gr += 1;
                self.combo += 1;
                self.ex_score += 1;
            }
            Judgement::Gd => {
                self.gd += 1;
                self.combo += 1;
            }
            Judgement::Bd => {
                self.bd += 1;
                self.combo = 0;
                self.combo_break += 1;
            }
            Judgement::Pr => {
                self.pr += 1;
                self.combo = 0;
                self.combo_break += 1;
            }
            Judgement::AirPoor => {
                self.air_poor += 1;
                // 不断连
            }
        }
        match dir {
            JudgeDir::Early => self.early += 1,
            JudgeDir::Late => self.late += 1,
            JudgeDir::Neutral => {}
        }
        self.max_combo = self.max_combo.max(self.combo);
    }

    /// 已判定音符数（不含空 POOR）。
    #[must_use]
    pub fn judged(&self) -> u32 {
        self.pg + self.gr + self.gd + self.bd + self.pr
    }
}

/// 血量条类型（beatoraja `GrooveGauge` 索引：ASSISTEASY=0 … EXHARDCLASS=8）。
///
/// 5k/7k 常用前 6 种；CLASS/EXCLASS/EXHARDCLASS 为段位（course）血条，当前无段位
/// 模式，保留参数供渲染/未来使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GaugeType {
    AssistEasy = 0,
    Easy = 1,
    Normal = 2,
    Hard = 3,
    ExHard = 4,
    Hazard = 5,
    Class = 6,
    ExClass = 7,
    ExHardClass = 8,
}

impl GaugeType {
    pub const ALL: [GaugeType; 9] = [
        GaugeType::AssistEasy,
        GaugeType::Easy,
        GaugeType::Normal,
        GaugeType::Hard,
        GaugeType::ExHard,
        GaugeType::Hazard,
        GaugeType::Class,
        GaugeType::ExClass,
        GaugeType::ExHardClass,
    ];

    /// 按 beatoraja 索引取值（设置持久化为 0-8 整数）。
    pub fn from_id(id: i64) -> Option<Self> {
        Self::ALL.into_iter().find(|t| *t as i64 == id)
    }

    /// 该类型在指定游玩模式下的判定参数（beatoraja `GaugeProperty.FIVEKEYS/SEVENKEYS`）。
    pub fn element(self, mode: crate::core::keybind::PlayMode) -> GaugeElement {
        let table: &[GaugeElement; 9] = match mode {
            crate::core::keybind::PlayMode::SevenKey => &SEVENKEYS,
            crate::core::keybind::PlayMode::FiveKey => &FIVEKEYS,
        };
        table[self as usize]
    }
}

/// 判定对血量的修正类型（beatoraja `GrooveGauge.GaugeModifier`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GaugeModifier {
    /// 回复量按 `total / totalNotes` 缩放（ASSIST/EASY/NORMAL）。
    Total,
    /// 回复上限由 TOTAL 值决定（HARD/EXHARD）。
    LimitIncrement,
    /// 伤害按 TOTAL 值/音符数放大（LR2 组与 5k ExHard 使用）。
    ModifyDamage,
    /// 无修正（HAZARD/CLASS 系列）。
    None,
}

/// 单种血条的判定参数（beatoraja `GaugeElementProperty`）。
#[derive(Debug, Clone, Copy)]
pub struct GaugeElement {
    pub modifier: GaugeModifier,
    /// 最低值（ASSIST/EASY/NORMAL 为 2：不 fail，只判合格；HARD 系为 0：归零 fail）。
    pub min: f32,
    pub max: f32,
    /// 初始值（%）。
    pub init: f32,
    /// 合格线（%）。
    pub border: f32,
    /// 判定影响 [PG, GR, GD, BD, PR, MS(空POOR)]（gauge 百分数，未经 modifier 修正）。
    pub value: [f32; 6],
    /// guts 低血量减伤表：(血量阈值, 伤害倍率)。
    pub guts: &'static [(f32, f32)],
}

// beatoraja GaugeProperty.FIVEKEYS / SEVENKEYS 参数表（原样移植）。
const FIVEKEYS: [GaugeElement; 9] = [
    GaugeElement { modifier: GaugeModifier::Total, min: 2.0, max: 100.0, init: 20.0, border: 50.0, value: [1.0, 1.0, 0.5, -1.5, -3.0, -0.5], guts: &[] },
    GaugeElement { modifier: GaugeModifier::Total, min: 2.0, max: 100.0, init: 20.0, border: 75.0, value: [1.0, 1.0, 0.5, -1.5, -4.5, -1.0], guts: &[] },
    GaugeElement { modifier: GaugeModifier::Total, min: 2.0, max: 100.0, init: 20.0, border: 75.0, value: [1.0, 1.0, 0.5, -3.0, -6.0, -2.0], guts: &[] },
    GaugeElement { modifier: GaugeModifier::LimitIncrement, min: 0.0, max: 100.0, init: 100.0, border: 0.0, value: [0.0, 0.0, 0.0, -5.0, -10.0, -5.0], guts: &[] },
    GaugeElement { modifier: GaugeModifier::ModifyDamage, min: 0.0, max: 100.0, init: 100.0, border: 0.0, value: [0.0, 0.0, 0.0, -10.0, -20.0, -10.0], guts: &[] },
    GaugeElement { modifier: GaugeModifier::None, min: 0.0, max: 100.0, init: 100.0, border: 0.0, value: [0.0, 0.0, 0.0, -100.0, -100.0, -100.0], guts: &[] },
    GaugeElement { modifier: GaugeModifier::None, min: 0.0, max: 100.0, init: 100.0, border: 0.0, value: [0.01, 0.01, 0.0, -0.5, -1.0, -0.5], guts: &[] },
    GaugeElement { modifier: GaugeModifier::None, min: 0.0, max: 100.0, init: 100.0, border: 0.0, value: [0.01, 0.01, 0.0, -1.0, -2.0, -1.0], guts: &[] },
    GaugeElement { modifier: GaugeModifier::None, min: 0.0, max: 100.0, init: 100.0, border: 0.0, value: [0.01, 0.01, 0.0, -2.5, -5.0, -2.5], guts: &[] },
];

const SEVENKEYS: [GaugeElement; 9] = [
    GaugeElement { modifier: GaugeModifier::Total, min: 2.0, max: 100.0, init: 20.0, border: 60.0, value: [1.0, 1.0, 0.5, -1.5, -3.0, -0.5], guts: &[] },
    GaugeElement { modifier: GaugeModifier::Total, min: 2.0, max: 100.0, init: 20.0, border: 80.0, value: [1.0, 1.0, 0.5, -1.5, -4.5, -1.0], guts: &[] },
    GaugeElement { modifier: GaugeModifier::Total, min: 2.0, max: 100.0, init: 20.0, border: 80.0, value: [1.0, 1.0, 0.5, -3.0, -6.0, -2.0], guts: &[] },
    GaugeElement { modifier: GaugeModifier::LimitIncrement, min: 0.0, max: 100.0, init: 100.0, border: 0.0, value: [0.15, 0.12, 0.03, -5.0, -10.0, -5.0], guts: &[(10.0, 0.4), (20.0, 0.5), (30.0, 0.6), (40.0, 0.7), (50.0, 0.8)] },
    GaugeElement { modifier: GaugeModifier::LimitIncrement, min: 0.0, max: 100.0, init: 100.0, border: 0.0, value: [0.15, 0.06, 0.0, -8.0, -16.0, -8.0], guts: &[] },
    GaugeElement { modifier: GaugeModifier::None, min: 0.0, max: 100.0, init: 100.0, border: 0.0, value: [0.15, 0.06, 0.0, -100.0, -100.0, -10.0], guts: &[] },
    GaugeElement { modifier: GaugeModifier::None, min: 0.0, max: 100.0, init: 100.0, border: 0.0, value: [0.15, 0.12, 0.06, -1.5, -3.0, -1.5], guts: &[(5.0, 0.4), (10.0, 0.5), (15.0, 0.6), (20.0, 0.7), (25.0, 0.8)] },
    GaugeElement { modifier: GaugeModifier::None, min: 0.0, max: 100.0, init: 100.0, border: 0.0, value: [0.15, 0.12, 0.03, -3.0, -6.0, -3.0], guts: &[] },
    GaugeElement { modifier: GaugeModifier::None, min: 0.0, max: 100.0, init: 100.0, border: 0.0, value: [0.15, 0.06, 0.0, -5.0, -10.0, -5.0], guts: &[] },
];

/// 按 modifier 修正单次判定影响（beatoraja `GaugeModifier` 语义）。
fn modify_value(f: f32, modifier: GaugeModifier, total_value: f32, total_notes: u32) -> f32 {
    match modifier {
        GaugeModifier::Total => {
            if f > 0.0 && total_notes > 0 {
                f * total_value / total_notes as f32
            } else {
                f
            }
        }
        GaugeModifier::LimitIncrement => {
            // 回复上限 = clamp((2*total - 320)/totalNotes, 0, 0.15)
            let pg = ((2.0 * total_value - 320.0) / total_notes.max(1) as f32).clamp(0.0, 0.15);
            if f > 0.0 { f * pg / 0.15 } else { f }
        }
        GaugeModifier::ModifyDamage => {
            if f >= 0.0 {
                return f;
            }
            // beatoraja MODIFY_DAMAGE：TOTAL 值 → 伤害倍率表；音符数 → fix2 惩罚
            const FIX1_TOTAL: [f64; 10] = [240.0, 230.0, 210.0, 200.0, 180.0, 160.0, 150.0, 130.0, 120.0, 0.0];
            const FIX1_TABLE: [f64; 10] = [1.0, 1.11, 1.25, 1.5, 1.666, 2.0, 2.5, 3.333, 5.0, 10.0];
            let mut i = 0;
            while i < FIX1_TOTAL.len() - 1 && f64::from(total_value) < FIX1_TOTAL[i] {
                i += 1;
            }
            let mut fix2 = 1.0f32;
            let mut note = 1000i32;
            let mut m = 0.002f32;
            while note > total_notes as i32 || note > 1 {
                fix2 += m * (note - total_notes.max(note as u32 / 2) as i32) as f32;
                note /= 2;
                m *= 2.0;
            }
            f * FIX1_TABLE[i].max(f64::from(fix2)) as f32
        }
        GaugeModifier::None => f,
    }
}

/// 血量条状态（beatoraja `GrooveGauge.Gauge` 模型，0-100 百分数）。
///
/// - ASSIST/EASY/NORMAL：init=20%、min=2%（最低 2% **不 fail**，合格线 = border）
/// - HARD/EXHARD/HAZARD/CLASS：init=100%、min=0%（**归零即 fail**，无合格线）
/// - 判定增量按类型参数 + modifier（TOTAL/LIMIT_INCREMENT）缩放，低血量触发 guts 减伤
#[derive(Resource, Debug, Clone)]
pub struct GaugeState {
    /// 当前血量（0-100）。
    pub value: f32,
    /// 是否已失败（HARD 系血条归零；ASSIST/EASY/NORMAL 不会 fail）。
    pub failed: bool,
    /// 血条类型。
    pub kind: GaugeType,
    /// 当前类型参数。
    pub element: GaugeElement,
    /// #TOTAL 值（modifier 缩放用）。
    pub total_value: f32,
    /// 可玩音符总数（modifier 缩放用）。
    pub total_notes: u32,
}

impl GaugeState {
    /// 创建血条（初始值 = 类型 init，如 NORMAL 20% / HARD 100%）。
    #[must_use]
    pub fn new(total_value: f32, total_notes: u32, kind: GaugeType, mode: crate::core::keybind::PlayMode) -> Self {
        let element = kind.element(mode);
        Self {
            value: element.init,
            failed: false,
            kind,
            element,
            total_value: total_value.max(1.0),
            total_notes,
        }
    }

    /// 记录一次判定对血量的影响（beatoraja `Gauge.update`，rate=1 为完整量）。
    pub fn record(&mut self, j: Judgement) {
        self.update(j, 1.0);
    }

    /// 按倍率记录判定对血量的影响（beatoraja `Gauge.update(judge, rate)`，
    /// HCN 持续回血用 rate=0.5 的 GR 增量）。
    pub fn update(&mut self, j: Judgement, rate: f32) {
        if self.failed {
            return;
        }
        let idx = match j {
            Judgement::Pg => 0,
            Judgement::Gr => 1,
            Judgement::Gd => 2,
            Judgement::Bd => 3,
            Judgement::Pr => 4,
            Judgement::AirPoor => 5,
        };
        let mut inc = modify_value(
            self.element.value[idx],
            self.element.modifier,
            self.total_value,
            self.total_notes,
        ) * rate;
        // guts：低血量时伤害减免
        if inc < 0.0 {
            for (t, m) in self.element.guts {
                if self.value < *t {
                    inc *= m;
                    break;
                }
            }
        }
        self.value = (self.value + inc).clamp(self.element.min, self.element.max);
        if self.value <= 0.0 {
            self.failed = true;
        }
    }

    /// 是否合格（HARD 系 border=0 → 只要未失败即合格；NORMAL 需 ≥ border）。
    #[must_use]
    #[allow(dead_code)] // 结算 / TIMER_GAUGE_MAX 使用
    pub fn is_qualified(&self) -> bool {
        self.value > 0.0 && self.value >= self.element.border
    }

    /// 是否满血。
    #[must_use]
    #[allow(dead_code)] // TIMER_GAUGE_MAX 使用
    pub fn is_max(&self) -> bool {
        self.value >= self.element.max
    }

    /// 合格线（%）。
    #[must_use]
    pub fn border(&self) -> f32 {
        self.element.border
    }

    /// 上限（%）。
    #[must_use]
    pub fn max(&self) -> f32 {
        self.element.max
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lr2_normal_windows() {
        let w = JudgeWindows::default(); // Normal
        assert_eq!(judge(0.0, &w), Some(Judgement::Pg));
        assert_eq!(judge(0.018, &w), Some(Judgement::Pg));
        assert_eq!(judge(0.030, &w), Some(Judgement::Gr));
        assert_eq!(judge(-0.050, &w), Some(Judgement::Gd)); // 50ms > GR 40
        assert_eq!(judge(0.080, &w), Some(Judgement::Gd));
        assert_eq!(judge(0.150, &w), Some(Judgement::Bd));
        assert_eq!(judge(0.200, &w), Some(Judgement::Bd));
        // 早按空 POOR（-200ms 之外，-1000ms 之内）
        assert_eq!(judge(-0.300, &w), Some(Judgement::AirPoor));
        assert_eq!(judge(-0.999, &w), Some(Judgement::AirPoor));
        // 太早忽略
        assert_eq!(judge(-1.100, &w), None);
        // 太晚由 miss 检测处理
        assert_eq!(judge(0.250, &w), None);
    }

    #[test]
    fn level_windows() {
        let vh = JudgeWindows::for_level(JudgeLevel::VeryHard);
        assert_eq!(judge(0.005, &vh), Some(Judgement::Pg)); // 8ms
        assert_eq!(judge(0.020, &vh), Some(Judgement::Gr)); // 24ms
        assert_eq!(judge(0.080, &vh), Some(Judgement::Bd)); // 40ms<80<=200
        assert_eq!(judge(-0.150, &vh), Some(Judgement::Bd)); // bd 窗口内
        assert_eq!(judge(-0.300, &vh), None); // 早于 early 100ms → 忽略
        let easy = JudgeWindows::for_level(JudgeLevel::Easy);
        assert_eq!(judge(0.010, &easy), Some(Judgement::Pg)); // 21ms
        assert_eq!(judge(0.030, &easy), Some(Judgement::Gr)); // 30ms > 21
    }

    #[test]
    fn state_accumulates() {
        let mut s = JudgeState::default();
        s.record(Judgement::Pg, JudgeDir::Early);
        s.record(Judgement::Pg, JudgeDir::Neutral);
        s.record(Judgement::Gr, JudgeDir::Late);
        s.record(Judgement::AirPoor, JudgeDir::Neutral); // 不断连
        s.record(Judgement::Pr, JudgeDir::Neutral); // 断连
        s.record(Judgement::Gd, JudgeDir::Early);
        assert_eq!(s.pg, 2);
        assert_eq!(s.gr, 1);
        assert_eq!(s.air_poor, 1);
        assert_eq!(s.pr, 1);
        assert_eq!(s.combo, 1); // Pr 后 Gd 恢复 1
        assert_eq!(s.max_combo, 3);
        assert_eq!(s.ex_score, 5); // 2+2+1
        assert_eq!(s.judged(), 5); // 不含空 POOR
        assert_eq!(s.early, 2, "Fast = 早判定数");
        assert_eq!(s.late, 1, "Slow = 晚判定数");
        assert_eq!(s.combo_break, 1, "断连次数");
    }

    use crate::core::keybind::PlayMode;

    #[test]
    fn gauge_element_tables_5k_7k() {
        // 5k/7k 参数组不同（beatoraja FIVEKEYS/SEVENKEYS）
        assert_eq!(GaugeType::Normal.element(PlayMode::SevenKey).border, 80.0);
        assert_eq!(GaugeType::Normal.element(PlayMode::FiveKey).border, 75.0);
        // ASSIST/EASY/NORMAL：init=20、min=2（不 fail）；HARD 系：init=100、min=0（归零 fail）
        for t in [GaugeType::AssistEasy, GaugeType::Easy, GaugeType::Normal] {
            let el = t.element(PlayMode::SevenKey);
            assert_eq!(el.init, 20.0);
            assert_eq!(el.min, 2.0);
        }
        for t in [GaugeType::Hard, GaugeType::ExHard, GaugeType::Hazard] {
            let el = t.element(PlayMode::SevenKey);
            assert_eq!(el.init, 100.0);
            assert_eq!(el.min, 0.0);
        }
        assert_eq!(GaugeType::from_id(2), Some(GaugeType::Normal));
        assert_eq!(GaugeType::from_id(9), None);
        // 5k 组 ExHard 用 MODIFY_DAMAGE（beatoraja EXHARD_5），7k 组用 LIMIT_INCREMENT
        assert_eq!(
            GaugeType::ExHard.element(PlayMode::FiveKey).modifier,
            GaugeModifier::ModifyDamage
        );
        assert_eq!(
            GaugeType::ExHard.element(PlayMode::SevenKey).modifier,
            GaugeModifier::LimitIncrement
        );
    }

    #[test]
    fn gauge_normal_does_not_fail() {
        // NORMAL（7k）：init=20%、border=80；PG 回复 = total/totalNotes（+0.1% for total=100/1000 notes）
        let mut g = GaugeState::new(100.0, 1000, GaugeType::Normal, PlayMode::SevenKey);
        assert_eq!(g.value, 20.0);
        assert!(!g.is_qualified(), "20% < border 80% 未合格");
        g.record(Judgement::Pg);
        assert!((g.value - 20.1).abs() < 1e-4, "PG +0.1% → {}", g.value);
        g.record(Judgement::Pr);
        assert!((g.value - 14.1).abs() < 1e-4, "PR -6% → {}", g.value);
        // NORMAL min=2：连续 miss 只会降到 2%，永不 fail
        for _ in 0..100 {
            g.record(Judgement::Pr);
        }
        assert_eq!(g.value, 2.0);
        assert!(!g.failed, "NORMAL 血条不 fail（beatoraja 语义）");
        // 全 PG 涨回并合格（2% + 900×0.1% = 92% > border 80）
        for _ in 0..900 {
            g.record(Judgement::Pg);
        }
        assert!(g.is_qualified(), "92% ≥ border 80% 应合格");
        assert!((g.value - 92.0).abs() < 0.1, "2 + 90 = 92% → {}", g.value);
    }

    #[test]
    fn gauge_hard_fails_on_zero_and_guts() {
        // HARD（7k）：init=100%、min=0、border=0（未失败即合格）
        let mut g = GaugeState::new(100.0, 1000, GaugeType::Hard, PlayMode::SevenKey);
        assert_eq!(g.value, 100.0);
        assert!(g.is_qualified(), "HARD border=0，满血合格");
        // BD 扣 5%（LIMIT_INCREMENT 下负值不缩放）
        g.record(Judgement::Bd);
        assert!((g.value - 95.0).abs() < 1e-4, "BD -5% → {}", g.value);
        // guts：低血量时伤害多档递减（<50 ×0.8、<40 ×0.7、<30 ×0.6、<20 ×0.5、<10 ×0.4）
        let mut g2 = GaugeState::new(100.0, 1000, GaugeType::Hard, PlayMode::SevenKey);
        for _ in 0..18 {
            g2.record(Judgement::Bd); // 100 → 20.5（前 10 次 -5 到 50，之后逐档减伤）
        }
        assert!(
            (g2.value - 20.5).abs() < 0.05,
            "18 次 BD 后 = 20.5（guts 减伤）→ {}",
            g2.value
        );
        // 无减伤时伤害大：value=100 时 BD 恰 -5
        let mut g_full = GaugeState::new(100.0, 1000, GaugeType::Hard, PlayMode::SevenKey);
        g_full.record(Judgement::Bd);
        assert!((g_full.value - 95.0).abs() < 1e-4, "满血时 BD -5 → {}", g_full.value);
        // 减伤生效：20.5 后 BD 仅 -3（20.5 ≥ 20，走 <30 档 ×0.6）
        g2.record(Judgement::Bd);
        assert!((g2.value - 17.5).abs() < 0.05, "<30 减伤 ×0.6 → {}", g2.value);
        // 归零 fail
        let mut g3 = GaugeState::new(100.0, 1000, GaugeType::Hard, PlayMode::SevenKey);
        for _ in 0..50 {
            g3.record(Judgement::Bd);
        }
        assert!(g3.failed, "HARD 归零即 fail");
        assert!(g3.value <= 0.0);
        // failed 后冻结
        let v = g3.value;
        g3.record(Judgement::Pg);
        assert_eq!(g3.value, v, "失败后血量冻结");
    }

    #[test]
    fn gauge_limit_increment_caps_recovery() {
        // LIMIT_INCREMENT：回复上限 pg = clamp((2*total-320)/notes, 0, 0.15)
        // total=100 → pg = (200-320)/1000 < 0 → 0 → HARD 不回复
        let mut g = GaugeState::new(100.0, 1000, GaugeType::Hard, PlayMode::SevenKey);
        g.record(Judgement::Pg);
        assert!((g.value - 100.0).abs() < 1e-4, "pg=0 时 PG 不回复");
        // total=200 → pg = (400-320)/1000 = 0.08 → PG 回复 0.08（0.15 × 0.08/0.15）
        let mut g2 = GaugeState::new(200.0, 1000, GaugeType::Hard, PlayMode::SevenKey);
        g2.record(Judgement::Bd); // 100 - 5 = 95
        g2.record(Judgement::Pg); // +0.08
        assert!((g2.value - 95.08).abs() < 1e-3, "PG +0.08% → {}", g2.value);
    }
}
