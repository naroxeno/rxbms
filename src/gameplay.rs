//! 游玩界面：下落式音符渲染 + 手动/自动判定 + BGM/键音播放。
//!
//! 玩法范围（用户约定）：7k/5k Single。
//! 判定：LR2 默认窗口（`#RANK` → 难度），见 [`judge`]。
//! 已实现：长音判定、血量条、皮肤化、统一设置系统。
//! TODO：结算界面、STOP 视觉精调、Box::leak 谱面泄漏。

pub mod bga;
pub mod chart;
pub mod data;
pub mod judge;
pub mod lane;
pub mod render;

use bevy::prelude::*;
use bms_rs::chart::prelude::*;
use gametime::{TimeSpan, TimeStamp};

use crate::{
    audio::{AudioLease, AudioManager},
    core::keybind::{KeyBindingsByMode, PlayMode},
    core::settings::SettingsStore,
    core::state::AppState,
    skin,
};

use self::{
    bga::BgaPlayer,
    chart::{LoadedChart, SelectedChart},
    data::GameplayData,
    judge::{GaugeState, GaugeType, JudgeDir, JudgeState, JudgeWindows, Judgement, judge},
    lane::{LaneStates, LnKind, release_ln, start_ln, update_ln},
    render::{GameplayRender, GameplayVisual, NoteRender},
};

/// 游玩界面插件。
pub struct GameplayPlugin;

/// 皮肤数据同步系统集：`sync_skin_state` 等把 gameplay 状态写入皮肤后，
/// `apply_skin_frame`（SkinPlugin）须在此之后执行，避免同帧读到旧值。
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SkinSyncSet;

impl Plugin for GameplayPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Gameplay), setup_gameplay)
            .add_systems(
                Update,
                (
                    tick_gameplay,
                    hold_update,
                    miss_detection,
                    manual_input_judge,
                    sync_gameplay_data,
                    sync_skin_state,
                    update_bga,
                )
                    .chain()
                    .in_set(SkinSyncSet)
                    .run_if(in_state(AppState::Gameplay)),
            )
            .add_systems(OnExit(AppState::Gameplay), teardown_gameplay);
    }
}

/// 一次游玩会话的运行时状态。
#[derive(Resource)]
pub struct GameplaySession {
    loaded: LoadedChart,
    /// 音频租约：占用的音频路径（退出时交还 `AudioManager`）。
    lease: AudioLease,
    player: ChartPlayer<'static>,
    started_at: TimeStamp,
    /// 判定窗口（按 `#RANK`）。
    judge_windows: JudgeWindows,
    /// 重建播放头所需的可见窗口（等待音频解码完成后重建基准时间）。
    visible_range: VisibleRangePerBpm,
    /// 是否在等待音频全部解码完成（期间不推进播放头、不判定）。
    loading: bool,
    /// 是否已请求退出（等待当前 BGM 自然播完再回选曲）。
    exiting: bool,
    /// 请求退出的时刻（超时兜底强制切）。
    exit_requested_at: TimeStamp,
    /// Auto 模式（F2 切换）：音符自动 PG，忽略输入。
    auto: bool,
    /// 当前游玩模式（由谱面键位决定，选择对应键位绑定）。
    pub mode: PlayMode,
}

/// 谱面加载失败标记（tick 中检测后返回选曲）。
#[derive(Resource)]
struct GameplayLoadFailed;

/// 退出时等待 BGM 播完的超时（秒，防止长采样卡住）。
const EXIT_TIMEOUT_SECS: f64 = 5.0;

// ---------- 界面构建 ----------

pub(crate) fn setup_gameplay(
    mut commands: Commands,
    selected: Res<SelectedChart>,
    mut audio: ResMut<AudioManager>,
    settings: Res<SettingsStore>,
    mut images: ResMut<Assets<Image>>,
) {
    info!("[gameplay] 加载谱面: {}", selected.title);
    let loaded = match LoadedChart::load(&selected.path) {
        Ok(loaded) => loaded,
        Err(e) => {
            error!("[gameplay] 加载失败: {e}");
            commands.insert_resource(GameplayLoadFailed);
            return;
        }
    };
    info!(
        "[gameplay] 音符 {} / 轨道 {} / 时长 {:.1}s / RANK {:?} / 音频 {} (BGM 事件 {} / 键音事件 {})",
        loaded.note_count(),
        loaded.lanes.len(),
        loaded.total_sec,
        loaded.rank,
        loaded.wav_paths.len(),
        loaded.bgm_event_count,
        loaded.keysound_event_count
    );

    // 向全局音频管理器请求加载：大 BGM 注册为流式（不缓存），其余（键音 +
    // 密集 BGM 采样）同步解码进缓存。注意：register_bgm 必须在 submit_priority
    // 之前（优先级加载会跳过流式 BGM 文件）
    audio.register_bgm(loaded.bgm_audio_stats());
    let paths: Vec<_> = loaded.wav_paths.values().cloned().collect();
    let lease = audio.acquire(&paths);
    let priority = loaded.priority_audio_paths(30.0);
    audio.submit_priority(priority.clone());
    info!(
        "[gameplay] 首批音频 {}/{}（前 30 秒，解码完成即开玩）",
        priority.len(),
        paths.len()
    );
    let reaction = TimeSpan::from_duration(std::time::Duration::from_secs_f64(0.5));
    let visible_range = VisibleRangePerBpm::new(loaded.chart.init_bpm(), reaction);
    let started_at = TimeStamp::now();
    let player = ChartPlayer::start(loaded.chart, visible_range.clone(), started_at);

    // 视觉渲染（背景/轨道/音符/数字/文本）全部由 Lua 皮肤（SkinRuntime）接管
    let render = GameplayRender::spawn(&mut commands, &loaded);

    // BGA 播放器（图片/视频背景动画）
    if !loaded.bga.is_empty() {
        info!(
            "[gameplay] BGA: {} 事件 / {} 图 / {} 视频",
            loaded.bga.events.len(),
            loaded.bga.images.len(),
            loaded.bga.videos.len()
        );
    }
    let bga_player = BgaPlayer::new(loaded.bga.clone(), &mut images);
    commands.insert_resource(bga_player);

    commands.insert_resource(GameplayData::from_chart(&loaded));
    commands.insert_resource(JudgeState::default());
    // 血条类型由设置决定（beatoraja GaugeType 0-8，默认 NORMAL=2）
    let gauge_type =
        GaugeType::from_id(settings.get_int("gauge_type", 2)).unwrap_or(GaugeType::Normal);
    let mode = loaded.play_mode();
    commands.insert_resource(GaugeState::new(loaded.total_value, loaded.note_count() as u32, gauge_type, mode));
    commands.insert_resource(LaneStates::default());
    let judge_windows = JudgeWindows::for_level(loaded.rank);
    commands.insert_resource(GameplaySession {
        loaded,
        lease,
        player,
        started_at,
        judge_windows,
        visible_range,
        loading: true,
        exiting: false,
        exit_requested_at: started_at,
        auto: false,
        mode,
    });
    commands.insert_resource(render);
    info!("[gameplay] 等待音频解码完成…");
}

// ---------- 主循环 ----------

/// 主循环：播放头推进、事件处理、渲染、退出判定。
#[allow(clippy::too_many_arguments)] // Bevy 系统参数
#[allow(clippy::collapsible_match)] // 限流 if 无法折叠进 match guard（spawned 是循环变量）
fn tick_gameplay(
    mut session: ResMut<GameplaySession>,
    mut judge_state: ResMut<JudgeState>,
    render: Res<GameplayRender>,
    mut next: ResMut<NextState<AppState>>,
    keys: Res<ButtonInput<KeyCode>>,
    load_failed: Option<Res<GameplayLoadFailed>>,
    mut note_q: Query<&mut NoteRender>,
    mut lanes: ResMut<LaneStates>,
    mut gauge: ResMut<GaugeState>,
    mut audio: ResMut<AudioManager>,
) {
    // 加载失败 → 回选曲
    if load_failed.is_some() {
        error!("[gameplay] 谱面加载失败，返回选曲");
        NextState::set_if_neq(&mut next, AppState::SongSelect);
        return;
    }

    // 完成铺面后的退出等待：等当前 BGM 自然播完再回选曲（超时兜底）。
    // 只有"完成铺面"会进入此分支；中途退出（ESC/失败）已直接释放音频。
    // 注意：若 BGM 已播完（短于谱面或未触发），下一帧即返回，不强制停留。
    if session.exiting {
        let now = TimeStamp::now();
        let timeout = now
            .elapsed_since(session.exit_requested_at)
            .as_secs_f64()
            > EXIT_TIMEOUT_SECS;
        if !audio.is_playing() || timeout {
            audio.stop_all(); // 兜底清理
            NextState::set_if_neq(&mut next, AppState::SongSelect);
        }
        return; // 等待期间不推进播放头、不判定
    }

    // 手动退出：加载中直接切；游玩中**立即释放**所有 gameplay 音频（不等待
    // BGM 播完——中途退出无等待语义，由 teardown 统一清理）。
    if keys.just_pressed(KeyCode::Escape) {
        info!("[gameplay] 手动退出（EX {} / Combo {}）", judge_state.ex_score, judge_state.combo);
        if session.loading {
            NextState::set_if_neq(&mut next, AppState::SongSelect);
        } else {
            audio.stop_all(); // 直接释放 BGM/键音/时钟，不等播完
            NextState::set_if_neq(&mut next, AppState::SongSelect);
        }
        return;
    }
    // Auto 切换
    if keys.just_pressed(KeyCode::F2) {
        session.auto = !session.auto;
        info!("[gameplay] Auto 模式: {}", if session.auto { "开" } else { "关" });
    }

    // 等待音频首批解码完成：期间不推进播放头、不判定（HUD 显示进度）
    if session.loading {
        if audio.is_ready() {
            info!("[gameplay] 首批音频就绪，开始游玩");
            // 启动谱面时钟（BGM 以音频线程时钟对齐开始），恢复轨道
            audio.begin_song(session.loaded.chart.init_bpm().as_f64());
            // 剩余音频交后台解码池渐进预加载（避免游玩中主线程现解卡顿）
            audio.start_low_loading();
            session.started_at = TimeStamp::now();
            session.player = ChartPlayer::start(
                session.loaded.chart,
                session.visible_range.clone(),
                session.started_at,
            );
            session.loading = false;
        } else {
            return;
        }
    }

    // 推进播放头，收集触发事件（音频经解码缓存 + mixer 并发 push，主线程零阻塞）
    let now = TimeStamp::now();
    let events = session.player.update(now);
    for e in &events {
        match &e.event {
            ChartEvent::Note {
                side: PlayerSide::Player1,
                wav_id,
                kind,
                ..
            } if *kind == NoteKind::Invisible => {
                // 隐形 note（3x/4x）：只触发键音，不参与判定（beatoraja 语义）
                if let Some(id) = wav_id
                    && let Some(path) = session.loaded.wav_paths.get(id)
                {
                    audio.play_synced(path);
                }
            }
            ChartEvent::Note {
                side: PlayerSide::Player1,
                wav_id,
                ..
            } if session.auto => {
                // Auto：自动 PG（并记录 last_hit，供 Lua 皮肤打击特效）
                if let Some(id) = wav_id
                    && let Some(path) = session.loaded.wav_paths.get(id)
                {
                    audio.play_synced(path);
                }
                if let Some(&idx) = session.loaded.note_by_event.get(&e.id.0) {
                    let key = session.loaded.notes[idx].key;
                    let hit_sec = now.elapsed_since(session.started_at).as_secs_f64();
                    lanes.lane(key).last_hit = Some((Judgement::Pg, hit_sec));
                    let is_ln = session.loaded.notes[idx].length.is_some();
                    if is_ln {
                        // LN：激活状态机（Auto 视为命中），不立即 consumed（等尾部完成）
                        let kind = LnKind::from(session.loaded.ln_mode);
                        let lane = lanes.lane(key);
                        start_ln(&mut lane.ln, idx, kind, Judgement::Pg, 0.0);
                    } else if let Ok(mut nr) = note_q.get_mut(render.note_entities[idx]) {
                        nr.consumed = true;
                    }
                }
                judge_state.record(Judgement::Pg, JudgeDir::Neutral);
                gauge.record(Judgement::Pg);
            }
            ChartEvent::Bgm { wav_id } => {
                if let Some(id) = wav_id
                    && let Some(path) = session.loaded.wav_paths.get(id)
                {
                    audio.play_synced(path);
                }
            }
            _ => {}
        }
    }

    // 血量归零 → 失败退出：立即释放所有 gameplay 音频（不等 BGM 播完）
    if gauge.failed {
        info!("[gameplay] 血量归零（GAUGE FAILED）");
        audio.stop_all();
        NextState::set_if_neq(&mut next, AppState::SongSelect);
        return;
    }

    // 谱面结束：全部音符已判定，或超时 → 进入完成等待，**等背景音乐自然播完**
    // 再回选曲（exiting 分支负责轮询），保留结尾演出画面。
    let elapsed = now.elapsed_since(session.started_at).as_secs_f64();
    let all_judged = judge_state.judged() >= session.loaded.note_count() as u32;
    if all_judged || elapsed > session.loaded.total_sec + 3.0 {
        info!(
            "[gameplay] 谱面结束（EX {} / Combo {}），等待 BGM 播完…",
            judge_state.ex_score, judge_state.combo
        );
        session.exiting = true;
        session.exit_requested_at = TimeStamp::now();
    }
}

/// 并行更新音符渲染（逐音符计算交给并行迭代器，主线程只读播放头）。
/// 长音保持推进：尾部到达 → 判尾（中途松手过）或完成（按住到尾）；
/// HCN 持有期间持续回血。
fn hold_update(
    session: Res<GameplaySession>,
    mut lanes: ResMut<LaneStates>,
    render: Res<GameplayRender>,
    mut judge_state: ResMut<JudgeState>,
    mut gauge: ResMut<GaugeState>,
    mut note_q: Query<&mut NoteRender>,
) {
    if session.loading || session.exiting {
        return;
    }
    let now_y = session.player.playback_state().progressed_y.0.as_f64();
    let now_sec = TimeStamp::now()
        .elapsed_since(session.started_at)
        .as_secs_f64();
    let mut done: Vec<Key> = Vec::new();
    for (key, lane) in lanes.iter_mut() {
        let Some(idx) = lane.ln.processing else {
            continue;
        };
        let note = &session.loaded.notes[idx];
        let Some(len) = note.length else {
            continue;
        };
        let head_y = note.position.0.as_f64();
        let len_y = len.as_f64();

        // HCN：持有期间持续回血（beatoraja：每 200ms 执行 gauge.update(GR, 0.5)——
        // 增量 = GR 回复值 ×0.5，随血条类型/TOTAL/音符数缩放）
        if lane.ln.kind == LnKind::HellChargeNote
            && now_sec - lane.ln.last_heal >= 0.20
        {
            lane.ln.last_heal = now_sec;
            gauge.update(Judgement::Gr, 0.5);
        }

        // 尾部到达判定
        match update_ln(&lane.ln, head_y, len_y, now_y) {
            None => {}
            Some(tail) => {
                if let Some(j) = tail {
                    // 中途松手 → 判尾（方向未知，记为 Neutral）
                    judge_state.record(j, JudgeDir::Neutral);
                    gauge.record(j);
                    lane.last_hit = Some((j, now_sec));
                }
                if let Ok(mut nr) = note_q.get_mut(render.note_entities[idx]) {
                    nr.consumed = true;
                }
                done.push(*key);
            }
        }
    }
    for k in done {
        let lane = lanes.lane(k);
        lane.ln.processing = None;
        lane.ln.lnend_judge = None;
        lane.ln.release_time = None;
    }
}

/// 音符经过未命中 → 普通 POOR（断连）。被 hold 中的 LN 跳过。
fn miss_detection(
    session: Res<GameplaySession>,
    render: Res<GameplayRender>,
    mut lanes: ResMut<LaneStates>,
    mut judge_state: ResMut<JudgeState>,
    mut gauge: ResMut<GaugeState>,
    mut note_q: Query<&mut NoteRender>,
) {
    if session.loading || session.auto || session.exiting {
        return;
    }
    let now_sec = TimeStamp::now()
        .elapsed_since(session.started_at)
        .as_secs_f64();
    let miss_after = session.judge_windows.bd_ms / 1000.0 + 0.05;
    for (i, note) in session.loaded.notes.iter().enumerate() {
        let Ok(mut nr) = note_q.get_mut(render.note_entities[i]) else {
            continue;
        };
        if nr.consumed {
            continue;
        }
        // 被活跃 LN 持有的音符（head 已命中）跳过过期检测
        let is_held = lanes.holds_note(i);
        if is_held {
            continue;
        }
        if now_sec - note.activate_time > miss_after {
            nr.consumed = true;
            // 清掉该音符对应的 LN 状态（若有残留）
            for (_, lane) in lanes.iter_mut() {
                if lane.ln.processing == Some(i) {
                    lane.ln.processing = None;
                }
            }
            judge_state.record(Judgement::Pr, JudgeDir::Neutral);
            gauge.record(Judgement::Pr);
            // miss 弹字（POOR）数据源
            lanes.lane(note.key).last_hit = Some((Judgement::Pr, now_sec));
        }
    }
}

/// 手动输入判定：按键 → 命中窗口内最近音符 / 早按空 POOR。
#[allow(clippy::too_many_arguments)] // Bevy 系统参数
fn manual_input_judge(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<KeyBindingsByMode>,
    session: Res<GameplaySession>,
    render: Res<GameplayRender>,
    mut lanes: ResMut<LaneStates>,
    mut judge_state: ResMut<JudgeState>,
    mut gauge: ResMut<GaugeState>,
    mut note_q: Query<&mut NoteRender>,
    mut audio: ResMut<AudioManager>,
) {
    if session.loading || session.auto || session.exiting {
        return;
    }
    let now_sec = TimeStamp::now()
        .elapsed_since(session.started_at)
        .as_secs_f64();
    let w = session.judge_windows; // Copy

    for code in keys.get_just_pressed() {
        let Some(target) = bindings.for_mode(session.mode).target_for(*code) else {
            continue;
        };
        let key: Key = target.into();

        // 押し直し：该键有活跃 LN 且中途松过手 → 复活（重新按住），不做新判定
        let lane = lanes.lane(key);
        if lane.ln.processing.is_some() && lane.ln.release_time.is_some() {
            lane.ln.release_time = None;
            continue;
        }

        // 1. 找判定窗口内的最近音符
        let mut hit: Option<(usize, Judgement)> = None;
        let mut hit_delta = f64::INFINITY;
        // 2. 找早按音符（delta < -bd 且 >= -early）
        let mut early: Option<usize> = None;
        let mut early_delta = f64::INFINITY;

        for (i, n) in session.loaded.notes.iter().enumerate() {
            if n.key != key {
                continue;
            }
            let Ok(nr) = note_q.get(render.note_entities[i]) else {
                continue;
            };
            if nr.consumed {
                continue;
            }
            let delta = now_sec - n.activate_time;
            if delta.abs() < hit_delta
                && (-w.bd_ms..=w.bd_ms).contains(&(delta * 1000.0))
            {
                hit_delta = delta.abs();
                hit = Some((i, judge(delta, &w).expect("窗口内必有判定")));
            } else if delta < -w.bd_ms / 1000.0
                && delta >= -w.early_poor_ms / 1000.0
                && delta < early_delta
            {
                early_delta = delta;
                early = Some(i);
            }
        }

        if let Some((i, j)) = hit {
            let note = &session.loaded.notes[i];
            // Fast/Slow 方向：早按（delta<0）= Fast，晚按（delta>0）= Slow
            let dir = {
                let delta = now_sec - note.activate_time;
                if delta < 0.0 {
                    JudgeDir::Early
                } else if delta > 0.0 {
                    JudgeDir::Late
                } else {
                    JudgeDir::Neutral
                }
            };
            if let Some(id) = note.wav_id
                && let Some(path) = session.loaded.wav_paths.get(&id)
            {
                audio.play_synced(path);
            }
            let is_ln = note.length.is_some();
            let lane = lanes.lane(key);
            if is_ln {
                // LN head 命中 → 激活状态机（不标记 consumed，等尾部）
                let kind = LnKind::from(session.loaded.ln_mode);
                start_ln(&mut lane.ln, i, kind, j, 0.0);
            } else if let Ok(mut nr) = note_q.get_mut(render.note_entities[i]) {
                nr.consumed = true;
            }
            // 记录命中（判定弹字 + 打击特效数据源；普通/LN 都要）
            lane.last_hit = Some((j, now_sec));
            judge_state.record(j, dir);
            gauge.record(j);
        } else if early.is_some() {
            judge_state.record(Judgement::AirPoor, JudgeDir::Neutral);
            gauge.record(Judgement::AirPoor);
        }
    }

    // 长音释放：窗口内松手 → 待尾判；早松（超 Good 窗口）→ 立即判尾 POOR
    for code in keys.get_just_released() {
        let Some(target) = bindings.for_mode(session.mode).target_for(*code) else { continue };
        let key: Key = target.into();
        let lane = lanes.lane(key);
        let Some(idx) = lane.ln.processing else {
            continue;
        };
        let note = &session.loaded.notes[idx];
        // 尾部时间近似：head 时间 + length 对应的秒数（用尾部 y 相对播放头换算的秒数）
        let tail_sec = note.activate_time + ln_duration_sec(note, &session);
        let released = release_ln(&mut lane.ln, now_sec, tail_sec, &w);
        if let Some(j) = released {
            // 立即判尾（POOR）→ LN 结束
            lane.ln.processing = None;
            if let Ok(mut nr) = note_q.get_mut(render.note_entities[idx]) {
                nr.consumed = true;
            }
            judge_state.record(j, JudgeDir::Neutral);
            gauge.record(j);
            lane.last_hit = Some((j, now_sec));
        }
        // 窗口内松手：release_ln 已设置待尾判，等待 hold_update 判尾
    }
}

/// measure（YCoordinate）→ 播放头时间（秒）：按 BPM 变化点分段累积，
/// 对齐 bms-rs `calculate_cumulative_times`（`delta_y × 240 / bpm`，
/// 1 measure = 240/BPM 秒，4/4 拍）；STOP 不影响播放头推进，故不参与。
pub(crate) fn measure_seconds(
    head_y: f64,
    len_y: f64,
    bpm_changes: &[(f64, f64)],
    init_bpm: f64,
) -> f64 {
    let tail_y = head_y + len_y;
    let mut secs = 0.0;
    let mut prev = head_y;
    let mut bpm = init_bpm;
    for (y, b) in bpm_changes {
        if *y <= head_y {
            bpm = *b;
            continue;
        }
        if *y >= tail_y {
            break;
        }
        secs += (*y - prev) * 240.0 / bpm;
        prev = *y;
        bpm = *b;
    }
    secs + (tail_y - prev) * 240.0 / bpm
}

/// LN 尾部时间（秒）：从 head 到 tail 的播放头时间（BPM 分段累积）。
fn ln_duration_sec(note: &crate::gameplay::chart::NoteView, session: &GameplaySession) -> f64 {
    let Some(len) = note.length else {
        return 0.0;
    };
    let head_y = note.position.0.as_f64();
    // BPM 变化点（y, bpm），与播放头同源
    let mut bpm_changes: Vec<(f64, f64)> = Vec::new();
    for (y, flows) in session.loaded.chart.flow_events() {
        for f in flows {
            if let bms_rs::chart::prelude::FlowEvent::Bpm(b) = f {
                bpm_changes.push((y.as_f64(), b.as_f64()));
            }
        }
    }
    bpm_changes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    measure_seconds(head_y, len.as_f64(), &bpm_changes, session.loaded.chart.init_bpm().as_f64())
}

/// 判定弹出文字计时清理。
/// 同步游玩数据表（每帧从实时资源汇总）。
fn sync_gameplay_data(
    mut data: ResMut<GameplayData>,
    judge: Res<JudgeState>,
    gauge: Res<GaugeState>,
    session: Res<GameplaySession>,
) {
    // 已游玩时长：从播放头计算（游玩开始后）
    let duration = if session.loading {
        0.0
    } else {
        TimeStamp::now()
            .elapsed_since(session.started_at)
            .as_secs_f64()
            .min(session.loaded.total_sec)
    };
    data.sync(&judge, &gauge, &session);
    data.duration_sec = duration;
}

/// 同步 Lua 皮肤状态（每帧，`sync_gameplay_data` 之后）。
///
/// 把 gameplay 实时状态写入 `SkinRuntime.state`（`main_state` 数据源）：
/// 时间/播放头/BPM/分数/判定/血量 + 可见音符窗口 + 动画 timers
/// （PLAY/READY/JUDGE/KEYON/BOMB/HOLD/FULLCOMBO）+ 判定弹字 + 长音活动。
#[allow(clippy::too_many_arguments)]
fn sync_skin_state(
    session: Res<GameplaySession>,
    data: Res<GameplayData>,
    gauge: Res<GaugeState>,
    note_q: Query<&NoteRender>,
    render: Res<GameplayRender>,
    lanes: Res<LaneStates>,
    bindings: Res<KeyBindingsByMode>,
    keys: Res<ButtonInput<KeyCode>>,
    store: Res<SettingsStore>,
    mut runtime: Option<NonSendMut<crate::skin::runtime::SkinRuntime>>,
) {
    let Some(runtime) = runtime.as_deref_mut() else { return };
    let ps = session.player.playback_state();
    let now_y = ps.progressed_y.0.as_f64();
    let visible_y = session.player.visible_window_y(ps.current_speed).0.as_f64();
    let mut st = match runtime.state.write() {
        Ok(s) => s,
        Err(_) => return,
    };
    let now_ms = data.duration_sec * 1000.0;
    st.now_time_ms = now_ms;
    // 场景时间（含加载阶段）：驱动 lane-bg/gauge 等入场动画在 playstart 前完成
    st.scene_time_ms = TimeStamp::now()
        .elapsed_since(session.started_at)
        .as_secs_f64()
        * 1000.0;
    st.now_y = now_y;
    st.visible_y = visible_y;
    // 玩家下落速度倍率（settings scroll_speed）；#SPEED 谱面速度已由
    // progressed_y 推进体现，勿乘 current_speed 避免双重加倍
    st.hispeed = store.get_float("scroll_speed", 1.0) as f64;
    st.bpm_now = ps.current_bpm.as_f64();
    st.bpm_min = data.initial_bpm;
    st.bpm_max = data.initial_bpm;
    st.duration_sec = data.duration_sec;
    st.total_sec = data.total_sec;
    // scroll timeline（#SCROLL 绝对值变化点，beatoraja 逐段下落用）
    {
        st.scroll_timeline.clear();
        st.scroll_timeline.push((0.0, 1.0));
        for (y, flows) in session.loaded.chart.flow_events() {
            for f in flows {
                if let bms_rs::chart::prelude::FlowEvent::Scroll(s) = f {
                    st.scroll_timeline.push((y.as_f64(), s.as_f64()));
                }
            }
        }
    }
    st.title = data.title.clone();
    st.artist = data.artist.clone().unwrap_or_default();
    st.genre = data.genre.clone().unwrap_or_default();
    st.play_level = data.play_level.map(i64::from).unwrap_or(0);
    st.total_notes = data.total_notes as i64;
    st.ex_score = i64::from(data.ex_score);
    st.combo = i64::from(data.combo);
    st.max_combo = i64::from(data.max_combo);
    st.pg = i64::from(data.pg);
    st.gr = i64::from(data.gr);
    st.gd = i64::from(data.gd);
    st.bd = i64::from(data.bd);
    st.pr = i64::from(data.pr);
    st.fast = i64::from(data.early);
    st.slow = i64::from(data.late);
    st.combo_break = i64::from(data.combo_break);
    st.gauge = f64::from(data.gauge);
    st.gauge_type = gauge.kind as i32;
    st.gauge_border = f64::from(gauge.border());
    st.gauge_max = f64::from(gauge.max());
    st.failed = data.failed;
    st.started = data.started;
    st.auto = data.auto;
    // 全局皮肤选项更新（81=LOADED、32=AUTOPLAYOFF、80=NOW_LOADING）
    for (k, v) in runtime.config.op_map.iter_mut() {
        match *k {
            81 => *v = data.started,
            32 => *v = !data.auto,
            80 => *v = !data.started,
            _ => {}
        }
    }
    // 基础 timers（存**开启时刻**，scene_time 基准；动画时间 = scene_time - 开启时刻，
    // 对齐 beatoraja TimerManager：getTimer 返回开启时刻、getNowTime(id)=now-开启时刻）
    let scene_now = st.scene_time_ms;
    st.timers[skin::state::TIMER_PLAY] = if data.started { 0 } else { skin::state::TIMER_OFF };
    st.timers[skin::state::TIMER_READY] = if data.started {
        skin::state::TIMER_OFF
    } else {
        0
    };
    st.timers[skin::state::TIMER_FAILED] = if data.failed {
        scene_now as i64
    } else {
        skin::state::TIMER_OFF
    };
    // 按键状态 + KEYON timer（按下时刻记录在 state，跨帧保持）
    let bindings = bindings.for_mode(session.mode);
    for (target, code) in bindings.entries() {
        let lane_idx = match target {
            crate::core::keybind::BindTarget::Scratch => 0,
            crate::core::keybind::BindTarget::Key(n) => n as usize,
        };
        if lane_idx >= st.keys.len() {
            continue;
        }
        let pressed = keys.pressed(code);
        st.keys[lane_idx] = pressed;
        if keys.just_pressed(code) {
            st.key_press_at[lane_idx] = scene_now; // 按下时刻（scene_time 基准）
        }
        if pressed {
            // 存开启时刻
            st.timers[skin::state::TIMER_KEYON_1P_SCRATCH + lane_idx] = st.key_press_at[lane_idx] as i64;
        } else {
            st.timers[skin::state::TIMER_KEYON_1P_SCRATCH + lane_idx] = skin::state::TIMER_OFF;
        }
    }
    // 判定弹字 + BOMB/JUDGE timer（来自 LaneStates.last_hit）
    // - judge 弹字：只保留**最近一次判定**（连续判定不叠加，旧的立即消失）
    // - BOMB/keybeam：**每 lane 独立**（多押时各 lane 特效同时显示，beatoraja 语义）
    st.judge_pops.clear();
    let mut latest: Option<(usize, u8, f64)> = None;
    // BOMB timer 先全关（无判定的 lane 不残留旧特效）
    for l in 0..st.keys.len() {
        st.timers[skin::state::TIMER_BOMB_1P_SCRATCH + l] = skin::state::TIMER_OFF;
    }
    for (key, lane_state) in lanes.iter() {
        let lane_idx = match key {
            bms_rs::chart::prelude::Key::Scratch(_) => 0,
            bms_rs::chart::prelude::Key::Key(n) => usize::from(*n),
            _ => continue,
        };
        if let Some((j, at_sec)) = lane_state.last_hit {
            let at_ms = at_sec * 1000.0;
            let elapsed = now_ms - at_ms;
            if elapsed >= 0.0 && elapsed < 1000.0 {
                // 最近判定（judge 弹字用）
                if latest.is_none_or(|(_, _, t)| at_ms > t) {
                    let judgement = match j {
                        Judgement::Pg => 0,
                        Judgement::Gr => 1,
                        Judgement::Gd => 2,
                        Judgement::Bd => 3,
                        Judgement::Pr | Judgement::AirPoor => 4,
                    };
                    latest = Some((lane_idx, judgement, at_ms));
                }
                // BOMB：该 lane 命中 500ms 内开启（多押各 lane 独立）。
                // 仅**真实击打**（PG/GR/GD/BD）；POOR/空 POOR 没按到键，不显示打击特效。
                if elapsed < 500.0
                    && !matches!(j, Judgement::Pr | Judgement::AirPoor)
                {
                    st.timers[skin::state::TIMER_BOMB_1P_SCRATCH + lane_idx] = at_ms as i64;
                }
                // Auto 模式：无物理按键，用判定事件模拟 KEYON（短按 150ms，keybeam）
                if data.auto && elapsed < 150.0 {
                    st.timers[skin::state::TIMER_KEYON_1P_SCRATCH + lane_idx] = at_ms as i64;
                }
            }
        }
    }
    if let Some((lane_idx, judgement, at_ms)) = latest {
        st.judge_pops.push(skin::state::JudgePop {
            lane: lane_idx,
            judgement,
            at_ms,
        });
        st.timers[skin::state::TIMER_JUDGE_1P] = at_ms as i64; // 开启时刻（判定时刻）
    } else {
        st.timers[skin::state::TIMER_JUDGE_1P] = skin::state::TIMER_OFF;
    }
    // FULLCOMBO timer（连击 > 0 视为 FC 进行中）
    st.timers[skin::state::TIMER_FULLCOMBO_1P] = if data.combo > 0 {
        0
    } else {
        skin::state::TIMER_OFF
    };
    // GAUGE_MAX timer（满血时开启，血条满血闪烁动画）
    st.timers[skin::state::TIMER_GAUGE_MAX_1P] = if gauge.is_max() {
        scene_now as i64
    } else {
        skin::state::TIMER_OFF
    };
    // 可见音符窗口：皮肤窗口 = 1/hispeed measure（beatoraja 1/hispeed×speed 可见范围，
    // 与 BPM 无关）；底部由 emit_notes 的 y 范围控制（判定线附近不提前消失）
    let window_measure = (1.0 / st.hispeed.max(0.1)).min(16.0);
    let window_top = now_y + window_measure;
    let note_count = session.loaded.notes.len().min(render.note_entities.len());
    st.notes.clear();
    for (i, n) in session.loaded.notes.iter().enumerate().take(note_count) {
        let pos = n.position.0.as_f64();
        // 只过滤窗口顶部；过线 note 由 emit_notes 的 y 范围控制（判定线附近不提前消失）
        if pos > window_top + 2.0 {
            continue;
        }
        let consumed = note_q
            .get(render.note_entities[i])
            .map(|nr| nr.consumed)
            .unwrap_or(false);
        let kind = match n.kind {
            bms_rs::chart::prelude::NoteKind::Long => 1,
            bms_rs::chart::prelude::NoteKind::Landmine => 2,
            bms_rs::chart::prelude::NoteKind::Invisible => 3,
            _ => 0,
        };
        let ln_active = lanes.holds_note(i);
        st.notes.push(skin::state::NoteState {
            idx: i,
            lane: n.lane,
            position: pos,
            length: n.length.map(|l| l.as_f64()),
            kind,
            consumed,
            ln_active,
        });
    }
    // HOLD timer：活跃长音列开启（0），其余关闭
    for (i, t) in st.timers.iter_mut().enumerate() {
        if (skin::state::TIMER_HOLD_1P_SCRATCH..skin::state::TIMER_HOLD_1P_SCRATCH + 8)
            .contains(&i)
        {
            let lane = i - skin::state::TIMER_HOLD_1P_SCRATCH;
            let active = lanes
                .iter()
                .any(|(k, s)| {
                    let li = match k {
                        bms_rs::chart::prelude::Key::Scratch(_) => 0,
                        bms_rs::chart::prelude::Key::Key(n) => usize::from(*n),
                        _ => usize::MAX,
                    };
                    li == lane && s.ln.processing.is_some()
                });
            *t = if active { 0 } else { skin::state::TIMER_OFF };
        }
    }
}

/// 血条渲染更新：宽度和颜色随血量变化。
/// BGA 更新：事件触发切换 + 视频帧推进（渲染由皮肤 destination 完成）。
fn update_bga(
    mut bga: ResMut<BgaPlayer>,
    session: Res<GameplaySession>,
    asset_server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
) {
    if session.loading || session.exiting {
        return;
    }
    let now_sec = TimeStamp::now()
        .elapsed_since(session.started_at)
        .as_secs_f64();
    bga.update(now_sec, &asset_server, &mut images);
}

/// HUD 更新：EX 分数 / 连击 / 判定计数 / Auto 状态。
// ---------- 清理 ----------

fn teardown_gameplay(
    mut commands: Commands,
    mut audio: ResMut<AudioManager>,
    session: Res<GameplaySession>,
    visuals: Query<Entity, With<GameplayVisual>>,
) {
    // 停止本谱面仍在播放的音频（BGM/键音），释放音频租约
    audio.stop_all();
    audio.release(&session.lease);
    for e in &visuals {
        commands.entity(e).despawn();
    }
    commands.remove_resource::<GameplaySession>();
    commands.remove_resource::<GameplayRender>();
    commands.remove_resource::<GameplayData>();
    commands.remove_resource::<JudgeState>();
    commands.remove_resource::<GaugeState>();
    commands.remove_resource::<LaneStates>();
    commands.remove_resource::<BgaPlayer>();
    commands.remove_resource::<GameplayLoadFailed>();
}
