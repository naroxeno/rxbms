//! 游玩数据表：一次游玩过程中产生的全部数据（曲目信息 / 判定统计 / 分数 / 血量 / 状态）。
//!
//! 用途：结算界面（Result）、游玩记录（Record）等直接消费本表，
//! 由 `sync` 每帧从实时资源（`JudgeState` / `GaugeState` / `GameplaySession`）汇总。

use bevy::prelude::*;
use bms_rs::bms::command::JudgeLevel;

use super::{
    chart::LoadedChart,
    judge::{GaugeState, JudgeState},
    GameplaySession,
};

/// 游玩数据表（Resource，一次游玩一份）。
///
/// 部分字段供后续结算界面/游玩记录使用（当前未消费，标注允许）。
#[derive(Resource, Clone, Debug)]
#[allow(dead_code)]
pub struct GameplayData {
    // ---- 曲目信息 ----
    /// 标题。
    pub title: String,
    /// 艺术家。
    pub artist: Option<String>,
    /// 流派。
    pub genre: Option<String>,
    /// 等级（#PLAYLEVEL）。
    pub play_level: Option<u8>,
    /// 判定难度（#RANK）。
    pub rank: JudgeLevel,
    /// #TOTAL（血量属性）。
    pub total_value: f32,
    /// 初始 BPM。
    pub initial_bpm: f64,
    /// 谱面时长（秒）。
    pub total_sec: f64,
    /// 可玩音符总数。
    pub total_notes: usize,
    /// 轨道数。
    pub lanes: usize,

    // ---- 判定统计 ----
    /// PGREAT / GREAT / GOOD / BAD / POOR / 空 POOR 计数。
    pub pg: u32,
    pub gr: u32,
    pub gd: u32,
    pub bd: u32,
    pub pr: u32,
    pub air_poor: u32,

    // ---- 分数 / 连击 ----
    /// 当前连击。
    pub combo: u32,
    /// 最大连击。
    pub max_combo: u32,
    /// EX 分数（2×PG + GR）。
    pub ex_score: u32,
    /// 理论最高 EX（2×total_notes）。
    pub max_ex: u32,
    /// 早判定计数（Fast）。
    pub early: u32,
    /// 晚判定计数（Slow）。
    pub late: u32,
    /// 断连次数（BD/PR）。
    pub combo_break: u32,

    // ---- 血量 ----
    /// 当前血量（0-100，beatoraja 语义）。
    pub gauge: f32,
    /// 是否失败（血量归零）。
    pub failed: bool,

    // ---- 游玩状态 ----
    /// 是否 Auto 模式。
    pub auto: bool,
    /// 是否已开始游玩（等待音频解码完成后为 true）。
    pub started: bool,
    /// 已游玩时间（秒）。
    pub duration_sec: f64,
    /// 已判定音符数（不含空 POOR）。
    pub judged: u32,
}

impl GameplayData {
    /// 从谱面初始化曲目信息与理论值。
    #[must_use]
    pub fn from_chart(chart: &LoadedChart) -> Self {
        Self {
            title: chart.title.clone(),
            artist: chart.artist.clone(),
            genre: chart.genre.clone(),
            play_level: chart.play_level,
            rank: chart.rank,
            total_value: chart.total_value,
            initial_bpm: chart.chart.init_bpm().as_f64(),
            total_sec: chart.total_sec,
            total_notes: chart.note_count(),
            lanes: chart.lanes.len(),
            pg: 0,
            gr: 0,
            gd: 0,
            bd: 0,
            pr: 0,
            air_poor: 0,
            combo: 0,
            max_combo: 0,
            ex_score: 0,
            max_ex: (chart.note_count() as u32).saturating_mul(2),
            early: 0,
            late: 0,
            combo_break: 0,
            gauge: 0.0,
            failed: false,
            auto: false,
            started: false,
            duration_sec: 0.0,
            judged: 0,
        }
    }

    /// 从实时资源同步统计（每帧调用，仅游玩模块内部使用）。
    pub(super) fn sync(&mut self, judge: &JudgeState, gauge: &GaugeState, session: &GameplaySession) {
        self.pg = judge.pg;
        self.gr = judge.gr;
        self.gd = judge.gd;
        self.bd = judge.bd;
        self.pr = judge.pr;
        self.air_poor = judge.air_poor;
        self.combo = judge.combo;
        self.max_combo = judge.max_combo;
        self.ex_score = judge.ex_score;
        self.early = judge.early;
        self.late = judge.late;
        self.combo_break = judge.combo_break;
        self.gauge = gauge.value;
        self.failed = gauge.failed;
        self.auto = session.auto;
        self.started = !session.loading;
        self.judged = judge.judged();
    }

    /// 分数比率（0-1，EX / 理论最高）。
    #[allow(dead_code)] // 结算界面使用
    #[must_use]
    pub fn score_ratio(&self) -> f64 {
        if self.max_ex == 0 {
            0.0
        } else {
            f64::from(self.ex_score) / f64::from(self.max_ex)
        }
    }

    /// 分数评级（AAA 8/9 … F，参考 LR2/萌百）。
    #[allow(dead_code)] // 结算界面使用
    #[must_use]
    pub fn grade(&self) -> &'static str {
        let r = self.score_ratio();
        match r {
            r if r >= 8.0 / 9.0 => "AAA",
            r if r >= 7.0 / 9.0 => "AA",
            r if r >= 6.0 / 9.0 => "A",
            r if r >= 5.0 / 9.0 => "B",
            r if r >= 4.0 / 9.0 => "C",
            r if r >= 3.0 / 9.0 => "D",
            r if r >= 2.0 / 9.0 => "E",
            _ => "F",
        }
    }
}
