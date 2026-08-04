//! 玩法状态桥：GameplaySession 快照 → Lua `main_state` API。
//!
//! `PlayState` 每帧由 gameplay 系统写入（`Arc<RwLock>` 共享），
//! `main_state` 的 `number(id)` / `timer(id)` 等闭包直接读取。
//! id 语义对齐 beatoraja `SkinProperty`（NUMBER_*/TIMER_* 常量）。

use std::sync::{Arc, RwLock};

use mlua::{Lua, Table};

/// timer 关闭值（beatoraja `TIMER_OFF_VALUE` = Long.MIN_VALUE）。
pub const TIMER_OFF: i64 = i64::MIN;

// ---- Timer id（beatoraja SkinProperty）----
pub const TIMER_PLAY: usize = 1;
pub const TIMER_READY: usize = 40;
/// 以下动画计时常量 M3b 使用（允许暂时未引用）。
#[allow(dead_code)]
pub const TIMER_FADEOUT: usize = 2;
#[allow(dead_code)]
pub const TIMER_FAILED: usize = 3;
#[allow(dead_code)]
pub const TIMER_JUDGE_1P: usize = 46;
#[allow(dead_code)]
pub const TIMER_FULLCOMBO_1P: usize = 48;
/// 满血 timer（beatoraja TIMER_GAUGE_MAX_1P = 44）。
#[allow(dead_code)]
pub const TIMER_GAUGE_MAX_1P: usize = 44;
#[allow(dead_code)]
pub const TIMER_BOMB_1P_SCRATCH: usize = 50;
#[allow(dead_code)]
pub const TIMER_HOLD_1P_SCRATCH: usize = 70;
#[allow(dead_code)]
pub const TIMER_KEYON_1P_SCRATCH: usize = 100;
#[allow(dead_code)]
pub const TIMER_KEYOFF_1P_SCRATCH: usize = 120;

/// 可见音符快照（每帧从 LoadedChart 提取可见窗口）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteState {
    /// LoadedChart.notes 全局下标（渲染槽身份用）。
    pub idx: usize,
    /// 轨道列索引（0..lanes.len()）。
    pub lane: usize,
    /// 谱面 y（YCoordinate）。
    pub position: f64,
    /// 长音长度（YCoordinate），非长音为 None。
    pub length: Option<f64>,
    /// 0=普通 1=长音 2=地雷 3=隐形。
    pub kind: u8,
    /// 已判定（隐藏）。
    pub consumed: bool,
    /// 长音 body 活动中（head 已判定、tail 未）。
    pub ln_active: bool,
}

/// 最近判定（judge 弹字动画）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JudgePop {
    pub lane: usize,
    /// 0=PG 1=GR 2=GD 3=BD 4=PR。
    pub judgement: u8,
    /// 判定发生时间（毫秒，相对游玩开始）。
    pub at_ms: f64,
}

/// 每帧同步的玩法状态快照（main_state 数据源）。
#[derive(Debug, Clone)]
pub struct PlayState {
    // ---- 时间 / 播放头 ----
    /// 已游玩毫秒（相对游玩开始；main_state.time()/timers 用）。
    pub now_time_ms: f64,
    /// 进入 Gameplay 以来的毫秒（含加载阶段；**无 timer 的入场动画用**——
    /// beatoraja 在 playstart 前播放完 lane-bg/gauge 等 loop 动画）。
    pub scene_time_ms: f64,
    /// 播放头 y（YCoordinate）。
    pub now_y: f64,
    /// 可见窗口高度（YCoordinate）。
    pub visible_y: f64,
    pub hispeed: f64,
    pub duration_sec: f64,
    /// 谱面总时长（秒，slider 进度用）。
    pub total_sec: f64,

    // ---- 曲目信息 ----
    pub title: String,
    pub artist: String,
    pub genre: String,
    pub play_level: i64,
    pub total_notes: i64,

    // ---- BPM ----
    pub bpm_now: f64,
    pub bpm_min: f64,
    pub bpm_max: f64,

    // ---- 分数 / 判定 ----
    pub ex_score: i64,
    pub combo: i64,
    pub max_combo: i64,
    pub pg: i64,
    pub gr: i64,
    pub gd: i64,
    pub bd: i64,
    pub pr: i64,
    /// Fast 计数（早判定）。
    pub fast: i64,
    /// Slow 计数（晚判定）。
    pub slow: i64,
    pub combo_break: i64,

    // ---- 血量 / 状态 ----
    /// 血量 0-100。
    pub gauge: f64,
    /// 血条类型（beatoraja GaugeType 索引 0-8，渲染选节点组用）。
    pub gauge_type: i32,
    /// 血条合格线（%）。
    pub gauge_border: f64,
    /// 血条上限（%）。
    pub gauge_max: f64,
    pub failed: bool,
    pub started: bool,
    pub auto: bool,

    // ---- 每帧动态 ----
    /// timer id → 毫秒（或 TIMER_OFF）。
    pub timers: [i64; 256],
    /// 按键按下状态（0=scratch, 1-7=key；M3b keybeam 用）。
    #[allow(dead_code)]
    pub keys: [bool; 8],
    /// 按键按下时刻（毫秒，KEYON timer 计算用）。
    pub key_press_at: [f64; 8],
    /// 可见音符。
    pub notes: Vec<NoteState>,
    /// scroll（#SCROLL）变化点 (measure, 绝对值)，按 measure 排序；
    /// 逐段下落（beatoraja 算法）：note 像素 = Σ(段 measure × 段 scroll) × region.h。
    pub scroll_timeline: Vec<(f64, f64)>,
    /// 最近判定（弹字动画；M3b judge 用）。
    #[allow(dead_code)]
    pub judge_pops: Vec<JudgePop>,
}

impl Default for PlayState {
    fn default() -> Self {
        Self {
            now_time_ms: 0.0,
            scene_time_ms: 0.0,
            now_y: 0.0,
            visible_y: 0.0,
            hispeed: 1.0,
            duration_sec: 0.0,
            total_sec: 0.0,
            title: String::new(),
            artist: String::new(),
            genre: String::new(),
            play_level: 0,
            total_notes: 0,
            bpm_now: 0.0,
            bpm_min: 0.0,
            bpm_max: 0.0,
            ex_score: 0,
            combo: 0,
            max_combo: 0,
            pg: 0,
            gr: 0,
            gd: 0,
            bd: 0,
            pr: 0,
            fast: 0,
            slow: 0,
            combo_break: 0,
            gauge: 0.0,
            gauge_type: 2, // NORMAL（beatoraja GaugeType）
            gauge_border: 80.0,
            gauge_max: 100.0,
            failed: false,
            started: false,
            auto: false,
            timers: [TIMER_OFF; 256],
            keys: [false; 8],
            key_press_at: [0.0; 8],
            scroll_timeline: vec![(0.0, 1.0)],
            notes: Vec::new(),
            judge_pops: Vec::new(),
        }
    }
}

/// 快捷读数（r/100 为百分比）。
impl PlayState {
    /// EX 分数比率（0-10000，万分比整数，对应 beatoraja `NUMBER_SCORE_RATE`）。
    pub fn rate_x100(&self) -> i64 {
        if self.total_notes == 0 {
            return 0;
        }
        let max = (self.total_notes * 2) as f64;
        (self.ex_score as f64 / max * 10000.0) as i64
    }
}

/// `main_state.number(id)`：beatoraja `NUMBER_*` 常量映射。
pub fn number(s: &PlayState, id: i64) -> i64 {
    match id {
        10 => (s.hispeed * 100.0) as i64,          // NUMBER_HISPEED_LR2（LR2 语义 ×100）
        14 => 0,                                    // NUMBER_LANECOVER1（M3 无 lane cover）
        71 => s.ex_score,                           // NUMBER_SCORE（M3 简化：EX 当 score）
        72 => 0,                                    // NUMBER_MAXSCORE（无理论最高记录）
        74 => s.total_notes,                        // NUMBER_TOTALNOTES
        75 => s.max_combo,                          // NUMBER_MAXCOMBO
        76 => s.pr,                                 // NUMBER_MISSCOUNT（miss = POOR 数）
        80 => s.pg,                                 // NUMBER_PERFECT2
        81 => s.gr,                                 // NUMBER_GREAT2
        82 => s.gd,                                 // NUMBER_GOOD2
        83 => s.bd,                                 // NUMBER_BAD2
        84 => s.pr,                                 // NUMBER_POOR2
        90 => s.bpm_max as i64,                     // NUMBER_MAXBPM（整数 BPM）
        91 => s.bpm_min as i64,                     // NUMBER_MINBPM（整数 BPM）
        96 => s.play_level,                         // NUMBER_PLAYLEVEL
        100 => s.ex_score,                          // NUMBER_POINT（总得分，M3 用 EX）
        101 => s.ex_score,                          // NUMBER_SCORE2
        102 => s.rate_x100() / 100,                 // NUMBER_SCORE_RATE 整数（%）
        103 => s.rate_x100() % 100,                 // NUMBER_SCORE_RATE_AFTERDOT 小数
        104 => s.combo,                             // NUMBER_COMBO（当前连击）
        105 => s.max_combo,                         // NUMBER_MAXCOMBO2
        106 => s.total_notes,                       // NUMBER_TOTALNOTES2
        107 => s.gauge as i64,                      // NUMBER_GROOVEGAUGE 整数（0-100）
        108 => 0,                                   // NUMBER_DIFF_EXSCORE（无 rival）
        110 => s.pg,                                // NUMBER_PERFECT
        111 => s.gr,                                // NUMBER_GREAT
        112 => s.gd,                                // NUMBER_GOOD
        113 => s.bd,                                // NUMBER_BAD
        114 => s.pr,                                // NUMBER_POOR
        151 => 0,                                   // NUMBER_TARGET_SCORE2（无 target）
        152 => 0,                                   // NUMBER_DIFF_HIGHSCORE（无 rival）
        155 => s.rate_x100() / 100,                 // NUMBER_SCORE_RATE2
        156 => s.rate_x100() % 100,                 // NUMBER_SCORE_RATE_AFTERDOT2
        160 => s.bpm_now as i64,                    // NUMBER_NOWBPM（整数 BPM）
        161 => (s.duration_sec / 60.0) as i64,      // NUMBER_PLAYTIME_MINUTE（已玩分钟）
        162 => (s.duration_sec as i64) % 60,        // NUMBER_PLAYTIME_SECOND（已玩秒）
        163 => ((s.total_sec - s.duration_sec + 1.0).max(0.0) / 60.0) as i64, // NUMBER_TIMELEFT_MINUTE
        164 => (((s.total_sec - s.duration_sec + 1.0).max(0.0)) as i64) % 60, // NUMBER_TIMELEFT_SECOND
        170 => 0,                                   // NUMBER_HIGHSCORE2（无记录）
        171 => s.ex_score,                          // NUMBER_SCORE3（本地游玩分数）
        173 => 0,                                   // NUMBER_TARGET_MAXCOMBO（无 target）
        174 => s.max_combo,                         // NUMBER_MAXCOMBO3
        176 => 0,                                   // NUMBER_TARGET_MISSCOUNT（无 target）
        177 => s.pr,                                // NUMBER_MISSCOUNT2
        310 => s.hispeed as i64,                    // NUMBER_HISPEED（整数部分，如 1.5 → 1）
        311 => ((s.hispeed * 100.0) as i64) % 100,  // NUMBER_HISPEED_AFTERDOT（小数，1.5 → 50）
        313 => 0,                                   // NUMBER_DURATION_GREEN
        314 => 0,                                   // NUMBER_LIFT1
        407 => (s.gauge % 1.0 * 100.0) as i64,      // NUMBER_GROOVEGAUGE_AFTERDOT
        420 => s.pr,                                // NUMBER_MISS
        423 => s.fast,                              // NUMBER_TOTALEARLY
        424 => s.slow,                              // NUMBER_TOTALLATE
        425 => s.combo_break,                       // NUMBER_COMBOBREAK
        426 => s.pr,                                // NUMBER_POOR_PLUS_MISS
        427 => s.bd + s.pr,                         // NUMBER_BAD_PLUS_POOR_PLUS_MISS
        1003 => 0,                                  // NUMBER_TABLE
        _ => 0,
    }
}

/// `main_state.float_number(id)`：beatoraja `FloatProperty` 简化（rate 等）。
pub fn float_number(s: &PlayState, id: i64) -> f64 {
    match id {
        102 => s.rate_x100() as f64 / 100.0, // NUMBER_SCORE_RATE
        _ => 0.0,
    }
}

/// `main_state.text(id)`：beatoraja `StringProperty`（10=title 13=genre 14=artist）。
pub fn text(s: &PlayState, id: i64) -> String {
    match id {
        10 => s.title.clone(),
        13 => s.genre.clone(),
        14 => s.artist.clone(),
        _ => String::new(),
    }
}

/// `main_state.timer(id)`：timer 毫秒值或 `TIMER_OFF`。
pub fn timer(s: &PlayState, id: i64) -> f64 {
    if id < 0 || id as usize >= s.timers.len() {
        return TIMER_OFF as f64;
    }
    s.timers[id as usize] as f64
}

/// 安装 `main_state` 模块（替换 M1 骨架）：闭包捕获 `Arc<RwLock<PlayState>>`。
pub fn install_main_state(lua: &Lua, state: Arc<RwLock<PlayState>>) -> mlua::Result<()> {
    let ms: Table = lua.create_table()?;

    let n = state.clone();
    ms.set(
        "number",
        lua.create_function(move |_, id: i64| {
            let s = n.read().map_err(|_| mlua::Error::external("state lock"))?;
            Ok(number(&s, id))
        })?,
    )?;
    let f = state.clone();
    ms.set(
        "float_number",
        lua.create_function(move |_, id: i64| {
            let s = f.read().map_err(|_| mlua::Error::external("state lock"))?;
            Ok(float_number(&s, id))
        })?,
    )?;
    let t = state.clone();
    ms.set(
        "text",
        lua.create_function(move |_, id: i64| {
            let s = t.read().map_err(|_| mlua::Error::external("state lock"))?;
            Ok(text(&s, id))
        })?,
    )?;
    let tm = state.clone();
    ms.set(
        "timer",
        lua.create_function(move |_, id: i64| {
            let s = tm.read().map_err(|_| mlua::Error::external("state lock"))?;
            Ok(timer(&s, id))
        })?,
    )?;
    let tt = state.clone();
    ms.set(
        "time",
        lua.create_function(move |_, ()| {
            let s = tt.read().map_err(|_| mlua::Error::external("state lock"))?;
            Ok(s.now_time_ms)
        })?,
    )?;
    let rt = state.clone();
    ms.set(
        "rate",
        lua.create_function(move |_, ()| {
            let s = rt.read().map_err(|_| mlua::Error::external("state lock"))?;
            Ok(s.rate_x100() as f64 / 100.0)
        })?,
    )?;
    let ex = state.clone();
    ms.set(
        "exscore",
        lua.create_function(move |_, ()| {
            let s = ex.read().map_err(|_| mlua::Error::external("state lock"))?;
            Ok(s.ex_score)
        })?,
    )?;
    let g = state.clone();
    ms.set(
        "gauge",
        lua.create_function(move |_, ()| {
            let s = g.read().map_err(|_| mlua::Error::external("state lock"))?;
            Ok(s.gauge)
        })?,
    )?;

    let pkg: Table = lua.globals().get("package")?;
    let loaded: Table = pkg.get("loaded")?;
    loaded.set("main_state", ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PlayState {
        PlayState {
            now_time_ms: 5000.0,
            now_y: 42.0,
            visible_y: 100.0,
            hispeed: 1.5,
            duration_sec: 125.0,
            total_sec: 300.0,
            title: "Test Title".into(),
            artist: "Artist".into(),
            genre: "Genre".into(),
            play_level: 10,
            total_notes: 500,
            bpm_now: 180.0,
            bpm_min: 100.0,
            bpm_max: 200.0,
            ex_score: 700,
            combo: 30,
            max_combo: 60,
            pg: 300,
            gr: 50,
            gd: 5,
            bd: 2,
            pr: 1,
            fast: 3,
            slow: 4,
            combo_break: 5,
            gauge: 75.0,
            failed: false,
            started: true,
            auto: false,
            timers: {
                let mut t = [TIMER_OFF; 256];
                t[TIMER_PLAY] = 5000;
                t[TIMER_READY] = 0;
                t[TIMER_JUDGE_1P] = 250;
                t
            },
            ..Default::default()
        }
    }

    #[test]
    fn number_mapping() {
        let s = sample();
        assert_eq!(number(&s, 71), 700);
        assert_eq!(number(&s, 74), 500);
        assert_eq!(number(&s, 75), 60);
        assert_eq!(number(&s, 76), 1);
        assert_eq!(number(&s, 80), 300);
        assert_eq!(number(&s, 81), 50);
        assert_eq!(number(&s, 82), 5);
        assert_eq!(number(&s, 83), 2);
        assert_eq!(number(&s, 84), 1);
        // BPM 为整数（beatoraja 语义，皮肤直接显示）
        assert_eq!(number(&s, 90), 200);
        assert_eq!(number(&s, 91), 100);
        assert_eq!(number(&s, 160), 180);
        assert_eq!(number(&s, 96), 10);
        assert_eq!(number(&s, 104), 30, "NUMBER_COMBO = 当前连击");
        assert_eq!(number(&s, 106), 500);
        assert_eq!(number(&s, 107), 75);
        assert_eq!(number(&s, 110), 300);
        assert_eq!(number(&s, 111), 50);
        assert_eq!(number(&s, 114), 1);
        // Hispeed 拆整数/小数（1.5 → "1" + "50"）
        assert_eq!(number(&s, 310), 1);
        assert_eq!(number(&s, 311), 50);
        assert_eq!(number(&s, 161), 2);
        assert_eq!(number(&s, 162), 5);
        assert_eq!(number(&s, 163), 2, "TIMELEFT = 剩余分钟（300-125+1=176s → 2）");
        assert_eq!(number(&s, 164), 56);
        assert_eq!(number(&s, 174), 60);
        assert_eq!(number(&s, 177), 1);
        assert_eq!(number(&s, 420), 1);
        assert_eq!(number(&s, 425), 5);
        assert_eq!(number(&s, 426), 1);
        assert_eq!(number(&s, 427), 3, "BAD+POOR = 2+1");
    }

    #[test]
    fn rate_and_gauge() {
        let s = sample();
        // ex=700 / (500*2) = 0.7 → 7000 万分比
        assert_eq!(s.rate_x100(), 7000);
        assert_eq!(number(&s, 102), 70);
        assert_eq!(number(&s, 103), 0);
        assert_eq!(number(&s, 107), 75);
    }

    #[test]
    fn timer_mapping() {
        let s = sample();
        assert_eq!(timer(&s, TIMER_PLAY as i64), 5000.0);
        assert_eq!(timer(&s, TIMER_JUDGE_1P as i64), 250.0);
        assert_eq!(timer(&s, 999), TIMER_OFF as f64);
        assert_eq!(timer(&s, TIMER_FULLCOMBO_1P as i64), TIMER_OFF as f64);
    }

    #[test]
    fn text_mapping() {
        let s = sample();
        assert_eq!(text(&s, 10), "Test Title");
        assert_eq!(text(&s, 13), "Genre");
        assert_eq!(text(&s, 14), "Artist");
        assert_eq!(text(&s, 99), "");
    }
}

#[cfg(test)]
mod lua_tests {
    use super::*;
    use mlua::Lua;

    #[test]
    fn lua_main_state_data() {
        let lua = Lua::new();
        let state = Arc::new(RwLock::new(PlayState {
            title: "TEST TITLE".into(),
            artist: "ARTIST".into(),
            genre: "GENRE".into(),
            ex_score: 1234,
            total_notes: 500,
            gauge: 50.0,
            ..Default::default()
        }));
        install_main_state(&lua, state.clone()).unwrap();
        let v: String = lua
            .load(r#"return require("main_state").text(10)"#)
            .eval()
            .unwrap();
        assert_eq!(v, "TEST TITLE");
        let a: String = lua
            .load(r#"return require("main_state").text(14)"#)
            .eval()
            .unwrap();
        assert_eq!(a, "ARTIST");
        let n: i64 = lua
            .load(r#"return require("main_state").number(100)"#)
            .eval()
            .unwrap();
        assert_eq!(n, 1234);
        let t: String = lua
            .load(r#"return require("main_state").text(1003)"#)
            .eval()
            .unwrap();
        assert_eq!(t, "", "table 文本无数据返回空");
    }
}
