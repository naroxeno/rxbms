//! 游玩结果：clear lamp（灯）与字母评级（beatoraja 规则）。

/// 游玩结果类型（"灯"，beatoraja `ClearType`）。
///
/// 按通关难度/方式排序：灯只升不降，游玩结束后取最高达成档。
/// `gauge_id` 为该灯对应的血条类型（beatoraja `GrooveGaugeType` 索引，见 gameplay gauge）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClearType {
    /// 未游玩。
    NoPlay = 0,
    /// 失败（血量归零）。
    Failed = 1,
    /// 辅助简单（Assist Easy 血条通关）。
    AssistEasy = 2,
    /// 轻辅助简单（Light Assist Easy）。
    LightAssistEasy = 3,
    /// 简单（Easy 血条通关）。
    Easy = 4,
    /// 普通（Normal 血条通关）。
    Normal = 5,
    /// 困难（Hard 血条通关）。
    Hard = 6,
    /// 极难（ExHard 血条通关）。
    ExHard = 7,
    /// 全连（Full Combo）。
    FullCombo = 8,
    /// 全完美（All Perfect / Perfect）。
    Perfect = 9,
    /// 最大值（Max，全 PG）。
    Max = 10,
}

impl ClearType {
    /// 按 beatoraja id 取灯（未知 → [`ClearType::NoPlay`]）。
    #[must_use]
    pub fn from_id(id: i64) -> Self {
        match id {
            1 => Self::Failed,
            2 => Self::AssistEasy,
            3 => Self::LightAssistEasy,
            4 => Self::Easy,
            5 => Self::Normal,
            6 => Self::Hard,
            7 => Self::ExHard,
            8 => Self::FullCombo,
            9 => Self::Perfect,
            10 => Self::Max,
            _ => Self::NoPlay,
        }
    }

    /// 该灯对应的血条类型（beatoraja `ClearType.gaugetype`）。
    #[must_use]
    pub fn gauge_ids(self) -> &'static [i64] {
        match self {
            Self::NoPlay | Self::Failed | Self::Perfect | Self::Max => &[],
            Self::AssistEasy => &[],
            Self::LightAssistEasy => &[0],
            Self::Easy => &[1],
            Self::Normal => &[2, 6],
            Self::Hard => &[3, 7],
            Self::ExHard => &[4, 8],
            Self::FullCombo => &[5],
        }
    }
}

/// 字母评级（beatoraja `rank[i] = rate >= i/27` 分档，rate = EX 达成率 %）。
///
/// 按 EX 分数达成率（0-100）划分：AAA ≥ 88.89%（24/27）、AA ≥ 77.78%（21/27）、
/// A ≥ 66.67%（18/27），此后每 3 分一档：B ≥ 55.56%、C ≥ 44.44%、D ≥ 33.33%、
/// E ≥ 22.22%，低于 E 为 F。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    /// AAA（达成率 ≥ 88.89%）。
    AAA,
    /// AA（≥ 77.78%）。
    AA,
    /// A（≥ 66.67%）。
    A,
    /// B（≥ 55.56%）。
    B,
    /// C（≥ 44.44%）。
    C,
    /// D（≥ 33.33%）。
    D,
    /// E（≥ 22.22%）。
    E,
    /// F（< 22.22%）。
    F,
}

impl Rank {
    /// 按达成率（0-100）评级。
    #[must_use]
    pub fn from_rate(rate: f64) -> Self {
        // 27 分制：i/27 档位（beatoraja `rank[i] = rate >= i/27`）
        const RANK_LEN: i64 = 27;
        let idx = ((rate / 100.0 * RANK_LEN as f64).floor() as i64).clamp(0, RANK_LEN);
        match idx {
            24.. => Self::AAA,
            21.. => Self::AA,
            18.. => Self::A,
            15.. => Self::B,
            12.. => Self::C,
            9.. => Self::D,
            6.. => Self::E,
            _ => Self::F,
        }
    }

    /// 按 EX 分数与最大 EX 评级。
    #[must_use]
    pub fn from_ex_score(ex: u32, max_ex: u32) -> Self {
        if max_ex == 0 {
            return Self::F;
        }
        Self::from_rate(ex as f64 / max_ex as f64 * 100.0)
    }

    /// 显示名。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::AAA => "AAA",
            Self::AA => "AA",
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::E => "E",
            Self::F => "F",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_thresholds() {
        // beatoraja 27 分制档位
        assert_eq!(Rank::from_rate(89.0), Rank::AAA);
        assert_eq!(Rank::from_rate(88.9), Rank::AAA);
        assert_eq!(Rank::from_rate(80.0), Rank::AA);
        assert_eq!(Rank::from_rate(70.0), Rank::A);
        assert_eq!(Rank::from_rate(60.0), Rank::B);
        assert_eq!(Rank::from_rate(50.0), Rank::C);
        assert_eq!(Rank::from_rate(40.0), Rank::D);
        assert_eq!(Rank::from_rate(30.0), Rank::E);
        assert_eq!(Rank::from_rate(10.0), Rank::F);
        // 边界：恰好 24/27 ≈ 88.888% → AAA；21/27 ≈ 77.778% → AA
        assert_eq!(Rank::from_rate(24.0 / 27.0 * 100.0), Rank::AAA);
        assert_eq!(Rank::from_rate(21.0 / 27.0 * 100.0), Rank::AA);
        // 0/满分
        assert_eq!(Rank::from_rate(0.0), Rank::F);
        assert_eq!(Rank::from_rate(100.0), Rank::AAA);
    }

    #[test]
    fn rank_from_ex_score() {
        assert_eq!(Rank::from_ex_score(0, 100), Rank::F);
        assert_eq!(Rank::from_ex_score(100, 100), Rank::AAA);
        assert_eq!(Rank::from_ex_score(90, 100), Rank::AAA);
        assert_eq!(Rank::from_ex_score(80, 100), Rank::AA);
        assert_eq!(Rank::from_ex_score(70, 100), Rank::A);
        assert_eq!(Rank::from_ex_score(0, 0), Rank::F, "最大 EX 为 0 → F");
    }

    #[test]
    fn clear_type_id_and_gauge() {
        assert_eq!(ClearType::from_id(0), ClearType::NoPlay);
        assert_eq!(ClearType::from_id(1), ClearType::Failed);
        assert_eq!(ClearType::from_id(5), ClearType::Normal);
        assert_eq!(ClearType::from_id(8), ClearType::FullCombo);
        assert_eq!(ClearType::from_id(10), ClearType::Max);
        assert_eq!(ClearType::from_id(99), ClearType::NoPlay, "未知 id → NoPlay");
        assert_eq!(ClearType::Normal.gauge_ids(), &[2, 6]);
        assert_eq!(ClearType::ExHard.gauge_ids(), &[4, 8]);
        assert!(ClearType::NoPlay.gauge_ids().is_empty());
    }
}
