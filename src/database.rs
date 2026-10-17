//! 铺面数据库模块：`songs.db`（SQLite）持久化 + 目录扫描。
//!
//! 架构：
//! - **配置**：用户在设置界面添加/移除铺面文件夹，目录列表存 `folders` 表。
//! - **索引**：扫描文件夹时用 `bms-rs` 解析每个铺面，元数据摘要 upsert 进 `songs` 表。
//! - **选曲**：选曲界面从 `songs` 表查询展示（不依赖内存缓存）。
//! - **增量**：按 (文件大小, mtime, 解析版本) 跳过未变化的铺面。
//! - **内容身份**：扫描时对文件内容算 FNV-1a 64 哈希，用于
//!   ①搬移/改名后仍保持同一条记录（不会重复计入）；②识别多个目录里的重复铺面。
//! - **健壮**：单个文件读/解析失败（含 bms-rs 内部 panic）只记入 `scan_errors`
//!   与报告，不中断整次扫描；已消失的铺面标记 `active = 0` 而不是删除，
//!   磁盘暂时不可用时不会丢记录。
//! - **性能**：并行目录枚举（`ignore`）+ 多线程解析 + 单事务批量写入；
//!   解析阶段不持有数据库锁，后台扫描时选曲界面照常可读。
//!
//! 数据库位置：`~/.rxbms/songs.db`。
//!
//! 编码：BMS 生态大量使用 Shift_JIS，而 `bms-rs` 只接受 UTF-8，
//! 读取时先按 UTF-8 尝试，失败则用 Shift_JIS 转码。

use std::{
    collections::{HashMap, HashSet},
    fmt, fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Instant, UNIX_EPOCH},
};

use bms_rs::bms::prelude::*;
use bevy::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};

use crate::core::state::AppState;

/// 铺面数据库插件。
pub struct SongDatabasePlugin;

impl Plugin for SongDatabasePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Startup), bootstrap_database);
    }
}

/// 元数据提取版本：解析逻辑/字段变化时 +1，会强制全库重解析。
pub const PARSE_VERSION: i64 = 1;

/// 表结构版本（`PRAGMA user_version`）。
const SCHEMA_VERSION: i64 = 2;

/// 单个铺面文件大小上限（超过视为异常文件，跳过并记录）。
const MAX_CHART_BYTES: u64 = 8 * 1024 * 1024;

/// 并行解析线程数上限（再高对磁盘/CPU 收益递减）。
const MAX_PARSE_THREADS: usize = 12;

/// 数据库写入批大小。
const WRITE_BATCH: usize = 512;

/// `~/.rxbms` 用户数据目录（songs.db、游玩记录、配置都放这里）。
#[must_use]
pub fn data_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| String::from("."));
    PathBuf::from(home).join(".rxbms")
}

/// 首次启动时的默认测试目录（无任何配置时自动添加）。
#[must_use]
fn default_test_folder() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(
        PathBuf::from(home).join(".local/share/lr2oraja/songs/rainbow_ogg"),
    )
}

/// 铺面元数据摘要（与 `songs` 表一一对应）。
#[derive(Debug, Clone, PartialEq)]
pub struct SongMeta {
    /// BMS 文件完整路径。
    pub path: PathBuf,
    /// 文件名（含扩展名）。
    pub file_name: String,
    /// `#TITLE`
    pub title: Option<String>,
    /// `#ARTIST`
    pub artist: Option<String>,
    /// `#GENRE`
    pub genre: Option<String>,
    /// `#PLAYLEVEL`
    pub play_level: Option<u8>,
    /// `#BPM`（初始 BPM）
    pub initial_bpm: Option<f64>,
    /// 全部音符数（含 BGM，不含空引用）。
    pub note_count: usize,
    /// 可玩音符数（通道 1x/2x 可见、3x/4x 隐形、5x/6x 长音）。
    pub playable_count: usize,
    /// BGM 音符数（通道 01）。
    pub bgm_count: usize,
    /// BPM 变化事件数（通道 03 / 08）。
    pub bpm_change_count: usize,
    /// `#STOP` 事件数（通道 09）。
    pub stop_count: usize,
    /// 总小节数（最后一个小节号 + 1）。
    pub measure_count: usize,
    /// 解析告警数。
    pub warning_count: usize,
    /// 文件字节数。
    pub size: u64,
    /// 文件内容的 FNV-1a 64 哈希（内容身份，不随路径变化）。
    pub content_hash: u64,
}

impl SongMeta {
    /// 列表行（含标题，CJK 字形由全局 Noto Sans CJK 字体保证）。
    #[must_use]
    pub fn list_line(&self) -> String {
        let title = self.title.as_deref().unwrap_or("(untitled)");
        format!(
            "{title}  [{}]  {}  Lv.{}  {}BPM  {} notes ({} playable)  {} measures",
            self.file_name,
            self.genre.as_deref().unwrap_or("?"),
            self.play_level.map_or_else(|| String::from("?"), |l| l.to_string()),
            self.initial_bpm.map_or_else(|| String::from("?"), |b| format!("{b:.0}")),
            self.note_count,
            self.playable_count,
            self.measure_count,
        )
    }
}

impl fmt::Display for SongMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let title = self.title.as_deref().unwrap_or("(untitled)");
        let artist = self.artist.as_deref().unwrap_or("(unknown)");
        let genre = self.genre.as_deref().unwrap_or("?");
        write!(
            f,
            "{}  「{title}」 by {artist}  [{} / Lv.{}]  {}BPM  {} notes ({} playable, {} bgm)  {} bpm-changes, {} stops, {} measures, {} warnings",
            self.file_name,
            genre,
            self.play_level.map_or_else(|| String::from("?"), |l| l.to_string()),
            self.initial_bpm.map_or_else(|| String::from("?"), |b| format!("{b:.0}")),
            self.note_count,
            self.playable_count,
            self.bgm_count,
            self.bpm_change_count,
            self.stop_count,
            self.measure_count,
            self.warning_count,
        )
    }
}

/// 一次扫描的统计摘要。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScanReport {
    /// 扫描的目录数。
    pub scanned_dirs: usize,
    /// 发现的铺面文件数（叶子规则过滤后）。
    pub files: usize,
    /// 新增的铺面数。
    pub added: usize,
    /// 更新（内容变化）的铺面数。
    pub updated: usize,
    /// 移除（文件已消失，已标记 inactive）的铺面数。
    pub removed: usize,
    /// 未变化跳过的铺面数。
    pub unchanged: usize,
    /// 解析/读取失败的铺面数。
    pub failed: usize,
    /// 与其他目录内容相同的重复铺面数（已从列表隐藏）。
    pub duplicates: usize,
    /// 扫描耗时（毫秒）。
    pub elapsed_ms: u128,
}

impl fmt::Display for ScanReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "扫描 {} 个目录 / {} 个文件（{}ms）：+{} 新增, ~{} 更新, -{} 移除, {} 未变, {} 重复, {} 失败",
            self.scanned_dirs,
            self.files,
            self.elapsed_ms,
            self.added,
            self.updated,
            self.removed,
            self.unchanged,
            self.duplicates,
            self.failed
        )
    }
}

/// `songs` 表中已有的行（增量判定用）。
#[derive(Debug, Clone)]
struct StoredRow {
    size: i64,
    mtime_ns: i64,
    parse_version: i64,
}

/// 文件扫描得到的事实（内容身份 + 增量判据）。
#[derive(Debug, Clone, Copy)]
struct FileFacts {
    size: i64,
    mtime_ns: i64,
    content_hash: i64,
}

/// 枚举阶段发现的候选文件。
#[derive(Debug, Clone)]
struct Candidate {
    path: PathBuf,
    size: i64,
    mtime_ns: i64,
}

/// 单个文件的解析结果。
enum ParseOutcome {
    Parsed { meta: SongMeta, facts: FileFacts },
    Failed { path: PathBuf, error: String },
}

/// songs.db 封装（Resource，可跨线程共享）。
///
/// rusqlite 的 `Connection` 是 `Send` 但非 `Sync`，用 `Arc<Mutex<_>>` 包装：
/// `Arc` 让后台扫描线程与 ECS 共享同一连接，`Mutex` 满足 Bevy Resource 的
/// `Send + Sync` 约束。扫描的解析阶段不持锁，只有短促的读写阶段持锁。
#[derive(Resource, Clone)]
pub struct SongsDb(Arc<Mutex<Connection>>);

impl SongsDb {
    /// 打开（必要时创建）`~/.rxbms/songs.db` 并初始化表结构。
    ///
    /// # Errors
    ///
    /// 目录创建或数据库打开/初始化失败时返回错误。
    pub fn open() -> Result<Self, String> {
        Self::open_at(data_dir().join("songs.db"))
    }

    /// 在指定路径打开（测试用）。
    ///
    /// # Errors
    ///
    /// 目录创建或数据库打开/初始化失败时返回错误。
    pub(crate) fn open_at(db_path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("创建数据目录失败 {}: {e}", parent.display()))?;
        }
        let conn = Connection::open(db_path).map_err(|e| format!("打开 songs.db 失败: {e}"))?;
        Self::init_connection(conn)
    }

    /// 打开仅存在于内存的数据库（磁盘数据库损坏时的降级路径）。
    ///
    /// # Errors
    ///
    /// 表结构初始化失败时返回错误。
    pub(crate) fn open_in_memory() -> Result<Self, String> {
        let conn =
            Connection::open_in_memory().map_err(|e| format!("创建内存数据库失败: {e}"))?;
        Self::init_connection(conn)
    }

    /// 统一的连接初始化：PRAGMA → 建表 → 迁移加列 → 建索引。
    ///
    /// 顺序很关键：**索引必须在迁移之后创建**。旧版数据库的 `songs` 表还没有
    /// `active` / `duplicate_of` 列，若在迁移前建索引，`CREATE INDEX` 会直接失败
    /// （实测：`no such column: active`），整次启动因此拿不到 `SongsDb` 资源。
    fn init_connection(conn: Connection) -> Result<Self, String> {
        // WAL：扫描写入不阻塞界面读取；NORMAL 同步：大库批量写入快数倍（崩溃最多丢最后一批）
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;
             PRAGMA busy_timeout = 5000;",
        )
        .map_err(|e| format!("配置 songs.db 失败: {e}"))?;
        conn.execute_batch(SCHEMA)
            .map_err(|e| format!("初始化 songs.db 失败: {e}"))?;
        migrate(&conn).map_err(|e| format!("迁移 songs.db 失败: {e}"))?;
        conn.execute_batch(SCHEMA_INDEXES)
            .map_err(|e| format!("创建索引失败: {e}"))?;
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    /// 查询全部铺面（按 Lv、标题排序），供选曲界面使用。
    ///
    /// 只返回「活跃且非重复」的记录：被删除/移走的文件标记 inactive，重复内容只留一份。
    ///
    /// # Errors
    ///
    /// SQL 查询失败时返回错误。
    pub fn list_songs(&self) -> Result<Vec<SongMeta>, String> {
        let conn = self.0.lock().map_err(|_| "songs.db 锁失效")?;
        let mut stmt = conn
            .prepare(
                "SELECT path, file_name, title, artist, genre, play_level, initial_bpm,
                        note_count, playable_count, bgm_count, bpm_change_count,
                        stop_count, measure_count, warning_count, size, content_hash
                 FROM songs
                 WHERE active = 1 AND duplicate_of IS NULL
                 ORDER BY play_level IS NULL, play_level, file_name",
            )
            .map_err(|e| format!("查询失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(SongMeta {
                    path: PathBuf::from(row.get::<_, String>(0)?),
                    file_name: row.get(1)?,
                    title: row.get(2)?,
                    artist: row.get(3)?,
                    genre: row.get(4)?,
                    play_level: row.get(5)?,
                    initial_bpm: row.get(6)?,
                    note_count: row.get::<_, i64>(7)? as usize,
                    playable_count: row.get::<_, i64>(8)? as usize,
                    bgm_count: row.get::<_, i64>(9)? as usize,
                    bpm_change_count: row.get::<_, i64>(10)? as usize,
                    stop_count: row.get::<_, i64>(11)? as usize,
                    measure_count: row.get::<_, i64>(12)? as usize,
                    warning_count: row.get::<_, i64>(13)? as usize,
                    size: row.get::<_, i64>(14)? as u64,
                    content_hash: row.get::<_, i64>(15)? as u64,
                })
            })
            .map_err(|e| format!("查询失败: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("读取行失败: {e}"))?);
        }
        Ok(out)
    }

    /// 查询已配置的铺面文件夹列表。
    ///
    /// # Errors
    ///
    /// SQL 查询失败时返回错误。
    pub fn list_folders(&self) -> Result<Vec<PathBuf>, String> {
        let conn = self.0.lock().map_err(|_| "songs.db 锁失效")?;
        let mut stmt = conn
            .prepare("SELECT path FROM folders ORDER BY path")
            .map_err(|e| format!("查询失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("查询失败: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(PathBuf::from(row.map_err(|e| format!("读取行失败: {e}"))?));
        }
        Ok(out)
    }

    /// 添加一个铺面文件夹（幂等，重复添加无效果）。
    ///
    /// # Errors
    ///
    /// SQL 写入失败时返回错误。
    pub fn add_folder(&self, path: &Path) -> Result<(), String> {
        // 规范化：去掉末尾分隔符、尽可能转绝对路径，保证与扫描出的子路径前缀一致
        let normalized = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf());
        let conn = self.0.lock().map_err(|_| "songs.db 锁失效")?;
        conn.execute(
            "INSERT OR IGNORE INTO folders (path) VALUES (?1)",
            params![normalized.to_string_lossy()],
        )
        .map_err(|e| format!("写入失败: {e}"))?;
        Ok(())
    }

    /// 移除一个铺面文件夹（同时删除该目录下所有已索引的铺面记录）。
    ///
    /// # Errors
    ///
    /// SQL 写入失败时返回错误。
    pub fn remove_folder(&self, path: &Path) -> Result<(), String> {
        let dir = path.to_string_lossy().into_owned();
        let conn = self.0.lock().map_err(|_| "songs.db 锁失效")?;
        conn.execute("DELETE FROM folders WHERE path = ?1", params![dir])
            .map_err(|e| format!("写入失败: {e}"))?;
        conn.execute(
            "DELETE FROM songs
             WHERE length(path) > length(?1) + 1
               AND substr(path, 1, length(?1) + 1) = ?1 || '/'",
            params![dir],
        )
        .map_err(|e| format!("写入失败: {e}"))?;
        conn.execute(
            "DELETE FROM scan_errors
             WHERE length(path) > length(?1) + 1
               AND substr(path, 1, length(?1) + 1) = ?1 || '/'",
            params![dir],
        )
        .map_err(|e| format!("写入失败: {e}"))?;
        Ok(())
    }

    /// 扫描全部已配置文件夹，增量更新 `songs` 表。
    ///
    /// # Errors
    ///
    /// 数据库事务失败时返回错误（单个文件失败只计入报告，不中止整体扫描）。
    pub fn scan(&self) -> Result<ScanReport, String> {
        let roots = self.list_folders()?;
        self.scan_roots(&roots)
    }

    /// 扫描指定的根目录（热重载按受影响目录增量重扫用）。
    ///
    /// # Errors
    ///
    /// 数据库事务失败时返回错误。
    pub fn scan_roots(&self, roots: &[PathBuf]) -> Result<ScanReport, String> {
        self.scan_roots_with_progress(roots, &mut |_, _| {})
    }

    /// 扫描指定根目录，并通过 `progress(已完成, 总数)` 报告解析进度。
    ///
    /// # Errors
    ///
    /// 数据库事务失败时返回错误。
    pub fn scan_roots_with_progress(
        &self,
        roots: &[PathBuf],
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<ScanReport, String> {
        let started = Instant::now();
        let roots = normalize_roots(roots);
        let mut report = ScanReport {
            scanned_dirs: roots.len(),
            ..Default::default()
        };
        if roots.is_empty() {
            report.elapsed_ms = started.elapsed().as_millis();
            return Ok(report);
        }

        // ---------- 1. 并行枚举（不持锁） ----------
        let (candidates, chart_dirs, walk_errors) = enumerate_chart_files(&roots);
        report.failed += walk_errors;
        let candidates = leaf_candidates(candidates, &chart_dirs);
        report.files = candidates.len();

        // ---------- 2. 增量判定（短暂持锁读已有行） ----------
        let stored = self.stored_rows()?;
        let mut unchanged = 0usize;
        let mut to_parse: Vec<PathBuf> = Vec::new();
        let mut on_disk: HashSet<String> = HashSet::with_capacity(candidates.len());
        for candidate in candidates {
            let key = candidate.path.to_string_lossy().into_owned();
            on_disk.insert(key.clone());
            match stored.get(&key) {
                Some(row)
                    if row.parse_version == PARSE_VERSION
                        && row.size == candidate.size
                        && row.mtime_ns == candidate.mtime_ns =>
                {
                    unchanged += 1;
                }
                _ => to_parse.push(candidate.path),
            }
        }
        report.unchanged = unchanged;

        // ---------- 3. 并行解析（不持锁） ----------
        let had_outcomes = !to_parse.is_empty();
        let outcomes = parse_parallel(&to_parse, MAX_PARSE_THREADS, progress);

        // ---------- 4. 单事务批量落库 ----------
        let now_sec = unix_secs_now();
        let mut added = 0usize;
        let mut updated = 0usize;
        let mut failed = 0usize;
        let mut duplicates = 0usize;
        {
            let mut conn = self.0.lock().map_err(|_| "songs.db 锁失效")?;
            let tx = conn
                .transaction()
                .map_err(|e| format!("开启事务失败: {e}"))?;

            let mut stale_paths: Vec<String> = Vec::new();
            // 4a0. 先标记已消失的文件为 inactive（必须早于 upsert：改名/搬移的
            // 旧记录此刻才处于 inactive，upsert 才能通过内容哈希「复活」它，
            // 保留以路径为键的游玩记录）
            // 4b. 已消失的文件：标记 inactive（保留记录，磁盘暂时不可用时不丢数据）
            for root in &roots {
                let dir_str = root.to_string_lossy().into_owned();
                let stale = {
                    let mut stmt = tx
                        .prepare_cached(
                            "SELECT path FROM songs
                             WHERE length(path) > length(?1) + 1
                               AND substr(path, 1, length(?1) + 1) = ?1 || '/'",
                        )
                        .map_err(|e| format!("查询失效记录失败: {e}"))?;
                    let rows = stmt
                        .query_map(params![dir_str], |row| row.get::<_, String>(0))
                        .map_err(|e| format!("查询失效记录失败: {e}"))?;
                    rows.collect::<Result<Vec<String>, _>>()
                        .map_err(|e| format!("读取失效记录失败: {e}"))?
                };
                let mut stmt = tx
                    .prepare_cached("UPDATE songs SET active = 0 WHERE path = ?1 AND active = 1")
                    .map_err(|e| format!("预编译失败: {e}"))?;
                for path in stale {
                    if !on_disk.contains(&path) {
                        stmt.execute(params![path])
                            .map_err(|e| format!("更新失败: {e}"))?;
                        // 此刻不计数：改名/搬移的旧记录可能马上被 upsert 复活
                        stale_paths.push(path);
                    }
                }
            }

            // 4a. 成功的解析结果：批量 upsert（内容哈希相同视为「未变化」，只刷新 mtime）
            let mut batch: Vec<(SongMeta, FileFacts)> = Vec::with_capacity(WRITE_BATCH);
            let flush = |batch: &mut Vec<(SongMeta, FileFacts)>,
                             added: &mut usize,
                             updated: &mut usize|
             -> Result<(), String> {
                if batch.is_empty() {
                    return Ok(());
                }
                let mut stmt = tx
                    .prepare_cached(UPSERT_SQL)
                    .map_err(|e| format!("预编译失败: {e}"))?;
                for (meta, facts) in batch.drain(..) {
                    let path_str = meta.path.to_string_lossy().into_owned();
                    let root = root_of(&roots, &meta.path)
                        .map(|r| r.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let mut is_new = !stored.contains_key(&path_str);
                    if is_new {
                        // 搬移/改名：同根目录下同内容且已失效的行直接改写路径，
                        // 这样游玩记录等以路径为键的数据不会因为改名而断掉
                        let resurrected: Option<String> = tx
                            .query_row(
                                "SELECT path FROM songs
                                 WHERE content_hash = ?1 AND active = 0 AND root = ?2
                                 LIMIT 1",
                                params![facts.content_hash, root],
                                |row| row.get(0),
                            )
                            .optional()
                            .map_err(|e| format!("查询失效记录失败: {e}"))?;
                        if let Some(old_path) = resurrected {
                            tx.execute(
                                "UPDATE songs SET path = ?1, root = ?2 WHERE path = ?3",
                                params![path_str, root, old_path],
                            )
                            .map_err(|e| format!("复活记录失败: {e}"))?;
                            is_new = false;
                        }
                    }
                    stmt.execute(params![
                        path_str,
                        root,
                        meta.file_name,
                        meta.title,
                        meta.artist,
                        meta.genre,
                        meta.play_level,
                        meta.initial_bpm,
                        meta.note_count as i64,
                        meta.playable_count as i64,
                        meta.bgm_count as i64,
                        meta.bpm_change_count as i64,
                        meta.stop_count as i64,
                        meta.measure_count as i64,
                        meta.warning_count as i64,
                        facts.size,
                        facts.mtime_ns,
                        facts.content_hash,
                        PARSE_VERSION,
                        now_sec,
                    ])
                    .map_err(|e| format!("写入失败: {e}"))?;
                    if is_new {
                        *added += 1;
                    } else {
                        *updated += 1;
                    }
                }
                Ok(())
            };

            for outcome in outcomes {
                match outcome {
                    ParseOutcome::Parsed { meta, facts } => {
                        batch.push((meta, facts));
                        if batch.len() >= WRITE_BATCH {
                            flush(&mut batch, &mut added, &mut updated)?;
                        }
                    }
                    ParseOutcome::Failed { path, error } => {
                        failed += 1;
                        tx.execute(
                            "INSERT INTO scan_errors (path, error, at) VALUES (?1, ?2, ?3)
                             ON CONFLICT(path) DO UPDATE SET error = excluded.error, at = excluded.at",
                            params![path.to_string_lossy(), error, now_sec],
                        )
                        .map_err(|e| format!("写入扫描错误失败: {e}"))?;
                    }
                }
            }
            flush(&mut batch, &mut added, &mut updated)?;

            // 4c. 重复内容：同一 content_hash 只保留路径最小的一条，其余隐藏
            tx.execute("UPDATE songs SET duplicate_of = NULL", [])
                .map_err(|e| format!("重置重复标记失败: {e}"))?;
            let dup_hashes: Vec<i64> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT content_hash FROM songs
                         WHERE active = 1 AND content_hash <> 0
                         GROUP BY content_hash HAVING COUNT(*) > 1",
                    )
                    .map_err(|e| format!("查询重复失败: {e}"))?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, i64>(0))
                    .map_err(|e| format!("查询重复失败: {e}"))?;
                rows.collect::<Result<Vec<i64>, _>>()
                    .map_err(|e| format!("读取重复失败: {e}"))?
            };
            let mut dup_groups: Vec<Vec<String>> = Vec::new();
            for hash in dup_hashes {
                let paths: Vec<String> = {
                    let mut stmt = tx
                        .prepare(
                            "SELECT path FROM songs
                             WHERE active = 1 AND content_hash = ?1
                             ORDER BY path",
                        )
                        .map_err(|e| format!("查询重复组失败: {e}"))?;
                    let rows = stmt
                        .query_map(params![hash], |row| row.get::<_, String>(0))
                        .map_err(|e| format!("查询重复组失败: {e}"))?;
                    rows.collect::<Result<Vec<String>, _>>()
                        .map_err(|e| format!("读取重复组失败: {e}"))?
                };
                dup_groups.push(paths);
            }
            {
                let mut set_dup = tx
                    .prepare("UPDATE songs SET duplicate_of = ?1 WHERE path = ?2 AND active = 1")
                    .map_err(|e| format!("预编译失败: {e}"))?;
                for paths in &dup_groups {
                    let Some((canonical, rest)) = paths.split_first() else {
                        continue;
                    };
                    for path in rest {
                        set_dup
                            .execute(params![canonical, path])
                            .map_err(|e| format!("标记重复失败: {e}"))?;
                        duplicates += 1;
                    }
                }
            }

            // 4c2. 真正失效的记录（未被「复活」到新路径的才算移除）
            {
                let mut stmt = tx
                    .prepare("SELECT active FROM songs WHERE path = ?1")
                    .map_err(|e| format!("查询失效记录失败: {e}"))?;
                for path in &stale_paths {
                    let active: Option<i64> = stmt
                        .query_row(params![path], |row| row.get(0))
                        .optional()
                        .map_err(|e| format!("查询失效记录失败: {e}"))?;
                    if active == Some(0) {
                        report.removed += 1;
                    }
                }
            }

            // 4d. 复扫成功的文件清理历史错误
            if had_outcomes {
                tx.execute(
                    "DELETE FROM scan_errors WHERE at < ?1",
                    params![now_sec],
                )
                .map_err(|e| format!("清理扫描错误失败: {e}"))?;
            }

            tx.commit().map_err(|e| format!("提交事务失败: {e}"))?;
        }

        report.added = added;
        report.updated = updated;
        report.failed += failed;
        report.duplicates = duplicates;
        report.elapsed_ms = started.elapsed().as_millis();
        Ok(report)
    }

    /// 读取已有行的增量判据（按 path）。
    fn stored_rows(&self) -> Result<HashMap<String, StoredRow>, String> {
        let conn = self.0.lock().map_err(|_| "songs.db 锁失效")?;
        let mut stmt = conn
            .prepare("SELECT path, size, mtime_ns, parse_version FROM songs")
            .map_err(|e| format!("查询失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    StoredRow {
                        size: row.get(1)?,
                        mtime_ns: row.get(2)?,
                        parse_version: row.get(3)?,
                    },
                ))
            })
            .map_err(|e| format!("查询失败: {e}"))?;
        let mut out = HashMap::new();
        for row in rows {
            let (path, stored) = row.map_err(|e| format!("读取行失败: {e}"))?;
            out.insert(path, stored);
        }
        Ok(out)
    }

    /// 最近一次扫描中失败的文件（供设置界面展示）。
    ///
    /// # Errors
    ///
    /// SQL 查询失败时返回错误。
    pub fn list_scan_errors(&self) -> Result<Vec<(PathBuf, String)>, String> {
        let conn = self.0.lock().map_err(|_| "songs.db 锁失效")?;
        let mut stmt = conn
            .prepare("SELECT path, error FROM scan_errors ORDER BY path LIMIT 500")
            .map_err(|e| format!("查询失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((PathBuf::from(row.get::<_, String>(0)?), row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("查询失败: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("读取行失败: {e}"))?);
        }
        Ok(out)
    }

    /// 活跃（非隐藏）铺面总数（日志 / 状态显示用）。
    ///
    /// # Errors
    ///
    /// SQL 查询失败时返回错误。
    pub fn count(&self) -> Result<usize, String> {
        let conn = self.0.lock().map_err(|_| "songs.db 锁失效")?;
        conn.query_row(
            "SELECT COUNT(*) FROM songs WHERE active = 1 AND duplicate_of IS NULL",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n as usize)
        .map_err(|e| format!("查询失败: {e}"))
    }
}

/// 表结构（全新库直接建好所有列/索引）。
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS folders (
    path TEXT PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS songs (
    path TEXT PRIMARY KEY,
    root TEXT NOT NULL DEFAULT '',
    file_name TEXT NOT NULL,
    title TEXT,
    artist TEXT,
    genre TEXT,
    play_level INTEGER,
    initial_bpm REAL,
    note_count INTEGER NOT NULL DEFAULT 0,
    playable_count INTEGER NOT NULL DEFAULT 0,
    bgm_count INTEGER NOT NULL DEFAULT 0,
    bpm_change_count INTEGER NOT NULL DEFAULT 0,
    stop_count INTEGER NOT NULL DEFAULT 0,
    measure_count INTEGER NOT NULL DEFAULT 0,
    warning_count INTEGER NOT NULL DEFAULT 0,
    size INTEGER NOT NULL DEFAULT 0,
    mtime_ns INTEGER NOT NULL DEFAULT 0,
    content_hash INTEGER NOT NULL DEFAULT 0,
    parse_version INTEGER NOT NULL DEFAULT 0,
    active INTEGER NOT NULL DEFAULT 1,
    duplicate_of TEXT,
    last_seen INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS scan_errors (
    path TEXT PRIMARY KEY,
    error TEXT NOT NULL,
    at INTEGER NOT NULL DEFAULT 0
);
"#;

/// 索引（必须在 [`migrate`] 之后创建：旧库此时才拥有 `active` / `duplicate_of` 等列）。
const SCHEMA_INDEXES: &str = r#"
CREATE INDEX IF NOT EXISTS idx_songs_active ON songs (active, duplicate_of);
CREATE INDEX IF NOT EXISTS idx_songs_hash ON songs (content_hash);
"#;

/// 批量 upsert（一次解析结果写入）。
const UPSERT_SQL: &str = r"
INSERT INTO songs (path, root, file_name, title, artist, genre, play_level, initial_bpm,
                   note_count, playable_count, bgm_count, bpm_change_count, stop_count,
                   measure_count, warning_count, size, mtime_ns, content_hash,
                   parse_version, last_seen, active, duplicate_of)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
        ?18, ?19, ?20, 1, NULL)
ON CONFLICT(path) DO UPDATE SET
    root = excluded.root,
    file_name = excluded.file_name,
    title = excluded.title,
    artist = excluded.artist,
    genre = excluded.genre,
    play_level = excluded.play_level,
    initial_bpm = excluded.initial_bpm,
    note_count = excluded.note_count,
    playable_count = excluded.playable_count,
    bgm_count = excluded.bgm_count,
    bpm_change_count = excluded.bpm_change_count,
    stop_count = excluded.stop_count,
    measure_count = excluded.measure_count,
    warning_count = excluded.warning_count,
    size = excluded.size,
    mtime_ns = excluded.mtime_ns,
    content_hash = excluded.content_hash,
    parse_version = excluded.parse_version,
    last_seen = excluded.last_seen,
    active = 1,
    duplicate_of = NULL
";

/// 旧库迁移：缺列就 ALTER 补齐，并删除已废弃的 `file_modified`。
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(songs)")?;
    let existing: HashSet<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<_, _>>()?;
    let wanted: [(&str, &str); 9] = [
        ("root", "TEXT NOT NULL DEFAULT ''"),
        ("warning_count", "INTEGER NOT NULL DEFAULT 0"),
        ("size", "INTEGER NOT NULL DEFAULT 0"),
        ("mtime_ns", "INTEGER NOT NULL DEFAULT 0"),
        ("content_hash", "INTEGER NOT NULL DEFAULT 0"),
        ("parse_version", "INTEGER NOT NULL DEFAULT 0"),
        ("active", "INTEGER NOT NULL DEFAULT 1"),
        ("duplicate_of", "TEXT"),
        ("last_seen", "INTEGER NOT NULL DEFAULT 0"),
    ];
    for (name, ddl) in wanted {
        if !existing.contains(name) {
            conn.execute_batch(&format!("ALTER TABLE songs ADD COLUMN {name} {ddl}"))?;
        }
    }
    if existing.contains("file_modified") {
        conn.execute_batch("ALTER TABLE songs DROP COLUMN file_modified")?;
    }
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

// ---------- 扫描辅助 ----------

/// 规范化根目录：转绝对路径、去重、丢弃被其它根目录包含的嵌套根（避免重复扫描）。
fn normalize_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::with_capacity(roots.len());
    for root in roots {
        let root = root.canonicalize().unwrap_or_else(|_| root.clone());
        if !out.iter().any(|existing| existing == &root) {
            out.push(root);
        }
    }
    // 若存在另一个根目录是 root 的祖先，则 root 重叠（会被重复扫描），丢弃
    out.iter()
        .filter(|root| {
            !out.iter()
                .any(|other| other != *root && root.starts_with(other))
        })
        .cloned()
        .collect()
}

/// 文件所属的配置根目录。
fn root_of<'a>(roots: &'a [PathBuf], path: &Path) -> Option<&'a PathBuf> {
    roots.iter().find(|root| path.starts_with(root))
}

/// 并行枚举所有 BMS 文件，同时收集「含铺面文件的目录」集合（叶子规则用）。
fn enumerate_chart_files(
    roots: &[PathBuf],
) -> (Vec<Candidate>, HashSet<PathBuf>, usize) {
    let candidates = Mutex::new(Vec::<Candidate>::new());
    let chart_dirs = Mutex::new(HashSet::<PathBuf>::new());
    let walk_errors = AtomicUsize::new(0);

    for root in roots {
        let mut builder = ignore::WalkBuilder::new(root);
        builder
            .standard_filters(false) // BMS 目录可能以 `.` 开头，不禁用隐藏/忽略规则
            .follow_links(false)
            .hidden(false)
            .threads(parse_threads());
        let candidates = &candidates;
        let chart_dirs = &chart_dirs;
        let walk_errors = &walk_errors;
        builder.build_parallel().run(|| {
            Box::new(move |entry| {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => {
                        walk_errors.fetch_add(1, Ordering::Relaxed);
                        return ignore::WalkState::Continue;
                    }
                };
                let path = entry.path();
                if !entry.file_type().is_some_and(|t| t.is_file()) || !is_chart_file(path) {
                    return ignore::WalkState::Continue;
                }
                let (size, mtime_ns) = entry
                    .metadata()
                    .ok()
                    .map_or((0, 0), |m| (m.len() as i64, mtime_ns_of(&m)));
                if let Some(parent) = path.parent() {
                    chart_dirs
                        .lock()
                        .expect("chart_dirs 锁失效")
                        .insert(parent.to_path_buf());
                }
                candidates
                    .lock()
                    .expect("candidates 锁失效")
                    .push(Candidate {
                        path: path.to_path_buf(),
                        size,
                        mtime_ns,
                    });
                ignore::WalkState::Continue
            })
        });
    }

    (
        candidates.into_inner().expect("candidates 锁失效"),
        chart_dirs.into_inner().expect("chart_dirs 锁失效"),
        walk_errors.into_inner(),
    )
}

/// 叶子规则：文件若存在「含有铺面的祖先目录」则不收录（父目录本身不算祖先）。
fn leaf_candidates(candidates: Vec<Candidate>, chart_dirs: &HashSet<PathBuf>) -> Vec<Candidate> {
    candidates
        .into_iter()
        .filter(|candidate| {
            let mut ancestor = candidate.path.parent().and_then(Path::parent);
            while let Some(dir) = ancestor {
                if chart_dirs.contains(dir) {
                    return false;
                }
                ancestor = dir.parent();
            }
            true
        })
        .collect()
}

/// 多线程解析：`AtomicUsize` 派发任务，主线程收集结果并回调进度。
fn parse_parallel(
    paths: &[PathBuf],
    max_threads: usize,
    progress: &mut dyn FnMut(usize, usize),
) -> Vec<ParseOutcome> {
    let total = paths.len();
    if total == 0 {
        return Vec::new();
    }
    let next = AtomicUsize::new(0);
    let (tx, rx) = std::sync::mpsc::channel::<ParseOutcome>();
    let mut outcomes = Vec::with_capacity(total);
    std::thread::scope(|scope| {
        let workers = parse_threads().min(max_threads).min(total).max(1);
        for _ in 0..workers {
            let tx = tx.clone();
            let next = &next;
            scope.spawn(move || {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(index) else {
                        break;
                    };
                    if tx.send(parse_one(path)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        for (done, outcome) in rx.into_iter().enumerate() {
            progress(done + 1, total);
            outcomes.push(outcome);
        }
    });
    outcomes
}

/// 解析单个文件。任何错误（含 bms-rs 内部 panic）都转成 `Failed`，绝不让扫描中断。
fn parse_one(path: &Path) -> ParseOutcome {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parse_file(path))) {
        Ok(Ok((meta, facts))) => ParseOutcome::Parsed { meta, facts },
        Ok(Err(error)) => ParseOutcome::Failed {
            path: path.to_path_buf(),
            error,
        },
        Err(_) => ParseOutcome::Failed {
            path: path.to_path_buf(),
            error: String::from("解析器 panic（文件可能损坏）"),
        },
    }
}

/// 读取 + 哈希 + 解码 + 解析单个铺面。
fn parse_file(path: &Path) -> Result<(SongMeta, FileFacts), String> {
    let metadata = fs::metadata(path).map_err(|e| format!("读取元数据失败: {e}"))?;
    if metadata.len() > MAX_CHART_BYTES {
        return Err(format!(
            "文件过大（{} 字节，上限 {MAX_CHART_BYTES}），已跳过",
            metadata.len()
        ));
    }
    let bytes = fs::read(path).map_err(|e| format!("读取失败: {e}"))?;
    let source = decode_bms(&bytes)?;
    let facts = FileFacts {
        size: i64::try_from(bytes.len()).unwrap_or(i64::MAX),
        mtime_ns: mtime_ns_of(&metadata),
        content_hash: content_hash(&bytes) as i64,
    };
    let meta = parse_meta(&source, path, bytes.len() as u64, facts.content_hash as u64)?;
    Ok((meta, facts))
}

/// 解析铺面源文本为元数据摘要。
fn parse_meta(
    source: &str,
    path: &Path,
    size: u64,
    content_hash: u64,
) -> Result<SongMeta, String> {
    let output = parse_bms(source, default_config());
    let bms = output.bms.map_err(|e| format!("解析失败: {e}"))?;

    // 音符统计（BMS 官方通道表：01=BGM，1x/2x=可见，3x/4x=隐形，5x/6x=长音）
    let mut note_count = 0;
    let mut playable_count = 0;
    let mut bgm_count = 0;
    for note in bms.notes().all_notes() {
        if note.wav_id.is_null() {
            continue;
        }
        note_count += 1;
        let ch = note.channel_id.to_string();
        if ch == "01" {
            bgm_count += 1;
        } else if matches!(
            ch.as_bytes().first(),
            Some(b'1' | b'2' | b'3' | b'4' | b'5' | b'6')
        ) {
            playable_count += 1;
        }
    }

    Ok(SongMeta {
        path: path.to_owned(),
        file_name: path
            .file_name()
            .map_or_else(|| String::from("?"), |n| n.to_string_lossy().into_owned()),
        title: bms.music_info.title.clone(),
        artist: bms.music_info.artist.clone(),
        genre: bms.music_info.genre.clone(),
        play_level: bms.metadata.play_level,
        initial_bpm: bms.bpm.bpm.as_ref().and_then(|v| v.raw().parse::<f64>().ok()),
        note_count,
        playable_count,
        bgm_count,
        bpm_change_count: bms.bpm.bpm_changes.len() + bms.bpm.bpm_changes_u8.len(),
        stop_count: bms.stop.stops.len(),
        measure_count: bms
            .last_obj_time()
            .map_or(0, |t| usize::try_from(t.track().0).unwrap_or(0) + 1),
        warning_count: output.warnings.len(),
        size,
        content_hash,
    })
}

/// 解析单个铺面文件（测试 / 单文件工具用）。
#[cfg(test)]
fn parse_chart(path: &Path) -> Result<SongMeta, String> {
    parse_file(path).map(|(meta, _)| meta)
}

/// 解析线程数：CPU 并行度（受 `MAX_PARSE_THREADS` 限制）。
fn parse_threads() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get).min(MAX_PARSE_THREADS)
}

/// 文件 mtime（纳秒精度，UNIX 纪元起）。
fn mtime_ns_of(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

/// 内容哈希：FNV-1a 64（够快且跨版本稳定，不依赖外部 crate）。
#[must_use]
pub fn content_hash(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

fn unix_secs_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Startup 阶段入口：打开 songs.db，首次启动时自动添加默认目录，
/// 后台扫描由 [`crate::library::SongScanner`] 接管（不阻塞进入选曲）。
fn bootstrap_database(
    mut commands: Commands,
    mut next: ResMut<NextState<AppState>>,
) {
    // 磁盘数据库打不开时降级到内存库：后续界面系统都依赖 `Res<SongsDb>`，
    // 直接 return 会让它们因「Resource does not exist」panic
    let db = match SongsDb::open() {
        Ok(db) => db,
        Err(e) => {
            error!("[song-database] 初始化失败: {e}；本次运行改用内存数据库（不持久化）");
            match SongsDb::open_in_memory() {
                Ok(db) => db,
                Err(e) => {
                    error!("[song-database] 内存数据库也无法创建: {e}；跳过曲库功能");
                    next.set(AppState::SongSelect);
                    return;
                }
            }
        }
    };

    // 首次启动：没有任何已配置目录时，添加默认测试目录
    if db.list_folders().is_ok_and(|f| f.is_empty())
        && let Some(default) = default_test_folder()
    {
        info!(
            "[song-database] 首次启动，添加默认测试目录: {}",
            default.display()
        );
        let _ = db.add_folder(&default);
    }

    match db.count() {
        Ok(n) => info!(
            "[song-database] songs.db 就绪，缓存 {n} 个铺面（点设置页「重新扫描」或选曲页 F5 更新）"
        ),
        Err(e) => warn!("[song-database] 读取计数失败: {e}"),
    }

    // 后台扫描 worker：启动时不自动扫描，只在用户显式刷新时工作
    let scanner = crate::library::SongScanner::spawn(db.clone());
    commands.insert_resource(scanner);
    commands.insert_resource(db);
    next.set(AppState::SongSelect);
}

/// 判断是否为受支持的铺面文件扩展名（不区分大小写）。
fn is_chart_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "bms" | "bme" | "bml" | "pms"
            )
        })
}

/// 把铺面源文本解码为 UTF-8。
///
/// 优先按 UTF-8 读取；失败（典型为 Shift_JIS 日文铺面）时用 Shift_JIS 转码。
pub(crate) fn decode_bms(bytes: &[u8]) -> Result<String, String> {
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok(s.to_owned()),
        Err(_) => {
            let (cow, had_errors) = encoding_rs::SHIFT_JIS.decode_without_bom_handling(bytes);
            let text = cow.into_owned();
            // 替换字符过多说明既不是 UTF-8 也不是 Shift_JIS（典型是二进制/加密文件）：
            // 这类文件交给 bms-rs 只会解析出空壳，不应入库
            if had_errors {
                let replaced = text.chars().filter(|c| *c == '\u{FFFD}').count();
                let total = text.chars().count().max(1);
                if replaced > 8 || replaced * 20 > total {
                    return Err(String::from(
                        "无法识别的编码（既非 UTF-8 也非 Shift_JIS）",
                    ));
                }
            }
            Ok(text)
        }
    }
}

#[cfg(test)]
mod bench_real {
    use super::*;

    #[test]
    #[ignore = "手动基准：合成铺面库"]
    fn bench_synthetic() {
        let root = std::env::temp_dir().join(format!("rxbms_bench_syn_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let n = 500;
        for i in 0..n {
            let dir = root.join(format!("song{i:04}"));
            fs::create_dir_all(&dir).unwrap();
            let body = format!(
                "#TITLE Song {i}\n#ARTIST Bench\n#BPM {}\n#PLAYLEVEL {}\n#WAV01 k.wav\n#00111:01010101\n#00101:01010101\n",
                120 + i % 60,
                i % 12
            );
            fs::write(dir.join("chart.bms"), body).unwrap();
        }
        let db_path = std::env::temp_dir().join(format!("rxbms_bench_syn_{}.db", std::process::id()));
        let _ = fs::remove_file(&db_path);
        let db = SongsDb::open_at(db_path.clone()).unwrap();
        let t0 = std::time::Instant::now();
        let report = db.scan_roots(std::slice::from_ref(&root)).unwrap();
        let first = t0.elapsed();
        let t1 = std::time::Instant::now();
        let report2 = db.scan_roots(std::slice::from_ref(&root)).unwrap();
        let second = t1.elapsed();
        println!("合成 {n} 铺面首扫: {report} / {first:?}");
        println!("增量: {report2} / {second:?}");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(&db_path);
    }

    #[test]
    #[ignore = "手动基准：扫描真实铺面库"]
    fn bench_real_library() {
        let root = PathBuf::from("/home/naroxeno/.local/share/lr2oraja/songs");
        if !root.is_dir() {
            return;
        }
        let db_path = std::env::temp_dir().join(format!("rxbms_bench_{}.db", std::process::id()));
        let _ = fs::remove_file(&db_path);
        let db = SongsDb::open_at(db_path.clone()).unwrap();
        let t0 = std::time::Instant::now();
        let report = db.scan_roots(std::slice::from_ref(&root)).unwrap();
        let first = t0.elapsed();
        let songs = db.list_songs().unwrap();
        let t1 = std::time::Instant::now();
        let report2 = db.scan_roots(std::slice::from_ref(&root)).unwrap();
        let second = t1.elapsed();
        let hwm = fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("VmHWM"))
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        println!("首扫: {report} / 用时 {first:?}");
        println!("增量: {report2} / 用时 {second:?}");
        println!("列表 {} 条 / {hwm}", songs.len());
        let _ = fs::remove_file(&db_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 迷你 BMS 文本：用于不依赖外部文件的解析测试。
    /// 通道 11 = 1P 可见音符，通道 01 = BGM。
    const MINI_BMS: &str = "\
#PLAYER 1
#GENRE TEST
#TITLE Mini Chart
#ARTIST Tester
#BPM 150
#PLAYLEVEL 5
#WAV01 test.wav
#00111:01
#00101:01
";

    /// 测试用的临时数据库路径（每个测试独立）。
    struct TempDb(PathBuf);

    impl TempDb {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("rxbms_test_{name}_{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn db_path(&self) -> PathBuf {
            self.0.join("songs.db")
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn parse_mini_bms() {
        let dir = std::env::temp_dir().join("rxbms_test_parse_mini");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mini.bms");
        fs::write(&path, MINI_BMS).unwrap();

        let meta = parse_chart(&path).expect("解析迷你铺面应成功");
        assert_eq!(meta.title.as_deref(), Some("Mini Chart"));
        assert_eq!(meta.artist.as_deref(), Some("Tester"));
        assert_eq!(meta.genre.as_deref(), Some("TEST"));
        assert_eq!(meta.play_level, Some(5));
        assert_eq!(meta.initial_bpm, Some(150.0));
        // 通道 11（可玩）与 01（BGM）各一个音符
        assert_eq!(meta.note_count, 2);
        assert_eq!(meta.playable_count, 1);
        assert_eq!(meta.bgm_count, 1);
        assert_eq!(meta.measure_count, 2);
        assert_eq!(meta.content_hash, content_hash(MINI_BMS.as_bytes()));
    }

    #[test]
    fn decode_shift_jis_text() {
        // 「ああああ」的 Shift_JIS 编码
        let sjis = [0x82u8, 0xA0, 0x82, 0xA0, 0x82, 0xA0, 0x82, 0xA0];
        assert_eq!(decode_bms(&sjis).expect("Shift_JIS 应可解码"), "ああああ");
        // UTF-8 原样保留
        assert_eq!(decode_bms("hello".as_bytes()).expect("UTF-8 应可解码"), "hello");
        // 二进制文件（UTF-8/Shift_JIS 都不合法）应被拒绝，不当作铺面
        assert!(decode_bms(&[0xff, 0xfe, 0x00, b'b', b'i', b'n']).is_err());
    }

    /// FNV-1a 已知向量：稳定性保证「搬移/改名后仍是同一铺面」。
    #[test]
    fn content_hash_known_value() {
        assert_eq!(content_hash(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(content_hash(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(content_hash(b"foobar"), 0x8594_4171_f739_67e8);
        // 单字节变化必须改变哈希
        assert_ne!(content_hash(b"#TITLE A"), content_hash(b"#TITLE B"));
    }

    /// 嵌套目录 + 叶子规则：含 BMS 的目录不再深入其子文件夹。
    #[test]
    fn nested_scan_stops_at_chart_dirs() {
        let temp = TempDb::new("nested");
        // root/a/x.bms + root/a/sub/y.bms + root/b/z.bms
        let root = temp.0.join("root");
        let a = root.join("a");
        let sub = a.join("sub");
        let b = root.join("b");
        for d in [&a, &sub, &b] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(a.join("x.bms"), MINI_BMS).unwrap();
        fs::write(sub.join("y.bms"), MINI_BMS.replace("Mini Chart", "Nested Chart")).unwrap();
        fs::write(b.join("z.bms"), MINI_BMS.replace("Mini Chart", "Other Chart")).unwrap();

        let db = SongsDb::open_at(temp.db_path()).expect("打开数据库");
        db.add_folder(&root).expect("添加根目录");
        let report = db.scan().expect("扫描");
        // 叶子规则：x（父目录 a 含 BMS）✓、z（父目录 b 含 BMS）✓；
        // y 在 a 的子目录 sub（sub 自身无 BMS，但其祖先 a 含 BMS）→ 排除
        assert_eq!(report.added, 2, "应只收录 x/z: {report}");

        let songs = db.list_songs().expect("查询铺面");
        let names: Vec<String> = songs.iter().map(|s| s.file_name.clone()).collect();
        assert!(names.contains(&"x.bms".to_string()));
        assert!(names.contains(&"z.bms".to_string()));
        assert!(!names.contains(&"y.bms".to_string()), "a/sub 不应被深入: {names:?}");
    }

    #[test]
    fn songs_db_roundtrip() {
        let temp = TempDb::new("roundtrip");

        // 准备一个临时铺面目录 + 迷你铺面
        let chart_dir = temp.0.join("charts");
        fs::create_dir_all(&chart_dir).unwrap();
        let chart = chart_dir.join("mini.bms");
        fs::write(&chart, MINI_BMS).unwrap();

        let db = SongsDb::open_at(temp.db_path()).expect("打开数据库");
        db.add_folder(&chart_dir).expect("添加目录");

        // 扫描 → 记录应存在且字段正确
        let report = db.scan().expect("扫描");
        assert_eq!(report.added, 1, "应新增 1 个铺面: {report}");
        assert_eq!(report.unchanged, 0);
        assert_eq!(report.files, 1);

        let songs = db.list_songs().expect("查询铺面");
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].title.as_deref(), Some("Mini Chart"));
        assert_eq!(songs[0].playable_count, 1);
        assert_eq!(songs[0].size, MINI_BMS.len() as u64);

        // 再次扫描 → 未变化，应全部跳过
        let report2 = db.scan().expect("二次扫描");
        assert_eq!(report2.unchanged, 1, "应跳过未变化文件: {report2}");
        assert_eq!(report2.added, 0);
        assert_eq!(report2.updated, 0);

        // 修改文件 → 再次扫描应更新
        fs::write(
            &chart,
            MINI_BMS.replace("#PLAYLEVEL 5", "#PLAYLEVEL 9"),
        )
        .unwrap();
        // 部分文件系统 mtime 精度不足，等 20ms 确保 mtime/大小变化
        std::thread::sleep(std::time::Duration::from_millis(20));
        let report3 = db.scan().expect("三次扫描");
        assert_eq!(report3.updated, 1, "内容变化应更新: {report3}");
        let songs = db.list_songs().expect("查询");
        assert_eq!(songs[0].play_level, Some(9));

        // 删除文件 → 再次扫描应标记 inactive（不再出现在列表里）
        fs::remove_file(&chart).unwrap();
        let report4 = db.scan().expect("四次扫描");
        assert_eq!(report4.removed, 1, "应移除消失的铺面: {report4}");
        assert!(db.list_songs().expect("查询").is_empty());
        assert_eq!(db.count().expect("计数"), 0);

        // 移除文件夹 → 配置消失
        db.remove_folder(&chart_dir).expect("移除目录");
        assert!(db.list_folders().expect("查询目录").is_empty());
    }

    /// 相同内容出现在两个目录：只展示一份（内容身份去重）。
    #[test]
    fn duplicate_content_listed_once() {
        let temp = TempDb::new("duplicate");
        let root = temp.0.join("root");
        let a = root.join("a");
        let b = root.join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("same.bms"), MINI_BMS).unwrap();
        fs::write(b.join("same.bms"), MINI_BMS).unwrap();

        let db = SongsDb::open_at(temp.db_path()).expect("打开数据库");
        db.add_folder(&root).expect("添加目录");
        let report = db.scan().expect("扫描");
        assert_eq!(report.added, 2);
        assert_eq!(report.duplicates, 1, "同内容应判为重复: {report}");

        let songs = db.list_songs().expect("查询");
        assert_eq!(songs.len(), 1, "重复铺面只展示一份: {songs:?}");
        // 保留路径字典序最小的那份
        assert_eq!(songs[0].path, a.join("same.bms").canonicalize().unwrap_or(a.join("same.bms")));
    }

    /// 搬移/改名后（同一内容）仍是同一条记录，不会重复计数。
    #[test]
    fn moved_file_keeps_identity() {
        let temp = TempDb::new("moved");
        let root = temp.0.join("root");
        fs::create_dir_all(&root).unwrap();
        let old = root.join("old.bms");
        fs::write(&old, MINI_BMS).unwrap();

        let db = SongsDb::open_at(temp.db_path()).expect("打开数据库");
        db.add_folder(&root).expect("添加目录");
        db.scan().expect("首扫");

        fs::rename(&old, root.join("new.bms")).unwrap();
        let report = db.scan().expect("二扫");
        assert_eq!(report.added, 0, "改名不应新增: {report}");
        assert_eq!(report.removed, 0, "同一内容改名不算移除: {report}");
        assert_eq!(report.updated, 1, "改名应更新原记录路径: {report}");
        let songs = db.list_songs().expect("查询");
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].file_name, "new.bms");
    }

    /// 解析版本变化会强制重解析（老库升级后元数据不会停在上个版本）。
    #[test]
    fn parse_version_forces_reparse() {
        let temp = TempDb::new("version");
        let root = temp.0.join("root");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("x.bms"), MINI_BMS).unwrap();

        let db = SongsDb::open_at(temp.db_path()).expect("打开数据库");
        db.add_folder(&root).expect("添加目录");
        db.scan().expect("首扫");

        {
            let conn = db.0.lock().unwrap();
            conn.execute("UPDATE songs SET parse_version = parse_version - 1", [])
                .unwrap();
        }
        let report = db.scan().expect("二扫");
        assert_eq!(report.unchanged, 0, "旧解析版本必须重扫: {report}");
        assert_eq!(report.updated, 1, "{report}");
    }

    /// 损坏文件不中断整次扫描，并记入 scan_errors。
    #[test]
    fn broken_file_does_not_abort_scan() {
        let temp = TempDb::new("broken");
        let root = temp.0.join("root");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("ok.bms"), MINI_BMS).unwrap();
        fs::write(root.join("broken.bms"), b"\xff\xfe\x00binary").unwrap();

        let db = SongsDb::open_at(temp.db_path()).expect("打开数据库");
        db.add_folder(&root).expect("添加目录");
        let report = db.scan().expect("扫描");
        assert_eq!(report.added, 1, "正常文件仍应入库: {report}");
        assert!(report.failed >= 1, "损坏文件应计入失败: {report}");
        let errors = db.list_scan_errors().expect("查询错误");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].0.ends_with("broken.bms"));
    }

    /// 大量文件并发扫描：结果完整（性能回归用，不断言耗时）。
    #[test]
    fn parallel_scan_many_files() {
        let temp = TempDb::new("many");
        let root = temp.0.join("root");
        let dir = root.join("pack");
        fs::create_dir_all(&dir).unwrap();
        let count = 120;
        for i in 0..count {
            let text = MINI_BMS.replace("Mini Chart", &format!("Chart {i}"));
            fs::write(dir.join(format!("c{i:03}.bms")), text).unwrap();
        }

        let db = SongsDb::open_at(temp.db_path()).expect("打开数据库");
        db.add_folder(&root).expect("添加目录");
        let report = db.scan().expect("扫描");
        assert_eq!(report.added, count as usize, "{report}");
        assert_eq!(db.count().expect("计数"), count as usize);
    }

    #[test]
    fn nested_roots_are_deduplicated() {
        let outer = std::env::temp_dir().join(format!("rxbms_nested_{}", std::process::id()));
        let inner = outer.join("inner");
        let _ = fs::remove_dir_all(&outer);
        fs::create_dir_all(&inner).unwrap();
        let roots = normalize_roots(&[outer.clone(), inner]);
        let _ = fs::remove_dir_all(&outer);
        assert_eq!(roots.len(), 1, "嵌套根目录只应扫描一次: {roots:?}");
        assert_eq!(roots[0], outer.canonicalize().unwrap_or(outer));
    }

    #[test]
    fn deleted_file_hidden_and_restored() {
        let tmp = TempDb::new("restore");
        let root = tmp.0.clone();
        let chart = root.join("a.bms");
        fs::write(&chart, MINI_BMS).unwrap();
        let db = SongsDb::open_at(tmp.db_path()).unwrap();
        db.scan_roots(std::slice::from_ref(&root)).unwrap();
        assert_eq!(db.count().unwrap(), 1);

        fs::remove_file(&chart).unwrap();
        let report = db.scan_roots(std::slice::from_ref(&root)).unwrap();
        assert_eq!(report.removed, 1, "删除应标为移除: {report}");
        assert_eq!(db.count().unwrap(), 0, "失效记录不应出现在列表");

        fs::write(&chart, MINI_BMS).unwrap();
        let report = db.scan_roots(std::slice::from_ref(&root)).unwrap();
        assert_eq!(report.updated, 1, "文件恢复应更新原记录（不新增）: {report}");
        assert_eq!(db.count().unwrap(), 1);
    }

    /// 旧版（v1）表结构必须能就地迁移：没有 `active` / `duplicate_of` 列的库
    /// 曾让 `CREATE INDEX` 报 `no such column: active`，整次启动拿不到 `SongsDb`。
    #[test]
    fn migrates_legacy_schema() {
        let tmp = TempDb::new("legacy_schema");
        let db_path = tmp.db_path();
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE folders (path TEXT PRIMARY KEY);
                 CREATE TABLE songs (
                     path TEXT PRIMARY KEY,
                     file_name TEXT NOT NULL,
                     title TEXT, artist TEXT, genre TEXT,
                     play_level INTEGER, initial_bpm REAL,
                     note_count INTEGER NOT NULL DEFAULT 0,
                     playable_count INTEGER NOT NULL DEFAULT 0,
                     bgm_count INTEGER NOT NULL DEFAULT 0,
                     bpm_change_count INTEGER NOT NULL DEFAULT 0,
                     stop_count INTEGER NOT NULL DEFAULT 0,
                     measure_count INTEGER NOT NULL DEFAULT 0,
                     warning_count INTEGER NOT NULL DEFAULT 0,
                     file_modified INTEGER NOT NULL DEFAULT 0
                 );
                 INSERT INTO songs (path, file_name, title)
                     VALUES ('/old/a.bms', 'a.bms', 'Old Chart');
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        }

        let db = SongsDb::open_at(db_path.clone()).expect("旧库应能打开并迁移");
        let songs = db.list_songs().expect("迁移后应可查询");
        assert_eq!(songs.len(), 1, "旧记录应保留且视为活跃");
        assert_eq!(songs[0].title.as_deref(), Some("Old Chart"));

        let conn = Connection::open(&db_path).unwrap();
        let columns: HashSet<String> = conn
            .prepare("PRAGMA table_info(songs)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for want in ["active", "duplicate_of", "content_hash", "parse_version"] {
            assert!(columns.contains(want), "迁移后缺少列 {want}");
        }
        assert!(!columns.contains("file_modified"), "旧列应被删除");
        let indexes: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE type = 'index' AND name IN ('idx_songs_active', 'idx_songs_hash')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(indexes, 2, "迁移后应建立两个索引");
    }
}
