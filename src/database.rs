//! 铺面数据库模块：`songs.db`（SQLite）持久化 + 目录扫描。
//!
//! 架构：
//! - **配置**：用户在设置界面添加/移除铺面文件夹，目录列表存 `folders` 表。
//! - **索引**：扫描文件夹时用 `bms-rs` 解析每个铺面，元数据摘要 upsert 进 `songs` 表。
//! - **选曲**：选曲界面从 `songs` 表查询展示（不依赖内存缓存）。
//! - **增量**：按文件修改时间跳过未变化的铺面；删除已消失的文件记录。
//!
//! 数据库位置：`~/.rxbms/songs.db`。
//!
//! 编码：BMS 生态大量使用 Shift_JIS，而 `bms-rs` 只接受 UTF-8，
//! 读取时先按 UTF-8 尝试，失败则用 Shift_JIS 转码。

use std::{
    fmt,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::UNIX_EPOCH,
};

use bms_rs::bms::prelude::*;
use bevy::prelude::*;
use rusqlite::{Connection, params};

use crate::core::state::AppState;

/// 铺面数据库插件。
pub struct SongDatabasePlugin;

impl Plugin for SongDatabasePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Startup), bootstrap_database);
    }
}

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
    /// 新增的铺面数。
    pub added: usize,
    /// 更新（内容变化）的铺面数。
    pub updated: usize,
    /// 移除（文件已消失）的铺面数。
    pub removed: usize,
    /// 未变化跳过的铺面数。
    pub unchanged: usize,
    /// 解析失败的铺面数。
    pub failed: usize,
}

impl fmt::Display for ScanReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "扫描 {} 个目录：+{} 新增, ~{} 更新, -{} 移除, {} 未变, {} 失败",
            self.scanned_dirs,
            self.added,
            self.updated,
            self.removed,
            self.unchanged,
            self.failed
        )
    }
}

/// songs.db 封装（Resource）。
///
/// rusqlite 的 `Connection` 是 `Send` 但非 `Sync`，用 `Mutex` 包装以满足
/// Bevy Resource 的 `Send + Sync` 约束。铺面量级下同步访问足够。
#[derive(Resource)]
pub struct SongsDb(Mutex<Connection>);

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
        conn.execute_batch(SCHEMA)
            .map_err(|e| format!("初始化 songs.db 失败: {e}"))?;
        Ok(Self(Mutex::new(conn)))
    }

    /// 查询全部铺面（按 Lv、标题排序），供选曲界面使用。
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
                        stop_count, measure_count, warning_count
                 FROM songs
                 ORDER BY play_level IS NULL, play_level, file_name",
            )
            .map_err(|e| format!("查询失败: {e}"))?;
        let rows = stmt
            .query_map([], row_to_meta)
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
        let conn = self.0.lock().map_err(|_| "songs.db 锁失效")?;
        conn.execute(
            "INSERT OR IGNORE INTO folders (path) VALUES (?1)",
            params![path.to_string_lossy()],
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
        Ok(())
    }

    /// 扫描全部已配置文件夹，增量更新 `songs` 表。
    ///
    /// 文件修改时间未变的铺面跳过；已消失的文件删除其记录。
    ///
    /// # Errors
    ///
    /// 事务失败时返回错误（已解析的单个文件失败只计入报告，不中止整体扫描）。
    pub fn scan(&self) -> Result<ScanReport, String> {
        let folders = self.list_folders()?;
        let mut report = ScanReport {
            scanned_dirs: folders.len(),
            ..Default::default()
        };
        let mut conn = self.0.lock().map_err(|_| "songs.db 锁失效")?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("开启事务失败: {e}"))?;

        for folder in &folders {
            scan_folder(&tx, folder, &mut report);
        }

        tx.commit().map_err(|e| format!("提交事务失败: {e}"))?;
        Ok(report)
    }

    /// 铺面总数（日志 / 状态显示用）。
    ///
    /// # Errors
    ///
    /// SQL 查询失败时返回错误。
    pub fn count(&self) -> Result<usize, String> {
        let conn = self.0.lock().map_err(|_| "songs.db 锁失效")?;
        conn.query_row("SELECT COUNT(*) FROM songs", [], |row| row.get::<_, i64>(0))
            .map(|n| n as usize)
            .map_err(|e| format!("查询失败: {e}"))
    }
}

/// 表结构。
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS folders (
    path TEXT PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS songs (
    path TEXT PRIMARY KEY,
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
    file_modified INTEGER NOT NULL DEFAULT 0
);
"#;

/// 从 SQL 行构造 `SongMeta`。
fn row_to_meta(row: &rusqlite::Row<'_>) -> rusqlite::Result<SongMeta> {
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
    })
}

/// 扫描单个目录：解析新/变化的铺面并 upsert，清理已消失的记录。
fn scan_folder(tx: &rusqlite::Transaction<'_>, folder: &Path, report: &mut ScanReport) {
    let dir_str = folder.to_string_lossy().into_owned();
    let Ok(entries) = fs::read_dir(folder) else {
        warn!("[song-database] 无法读取目录: {}", folder.display());
        report.failed += 1;
        return;
    };

    // 磁盘上实际存在的铺面路径集合
    let mut on_disk: std::collections::HashSet<String> = std::collections::HashSet::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || !is_chart_file(&path) {
            continue;
        }
        let path_str = path.to_string_lossy().into_owned();
        on_disk.insert(path_str.clone());

        // 增量：mtime 未变则跳过
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0));

        let unchanged = tx
            .query_row(
                "SELECT file_modified FROM songs WHERE path = ?1",
                params![path_str],
                |row| row.get::<_, i64>(0),
            )
            .is_ok_and(|stored| stored == mtime);
        if unchanged {
            report.unchanged += 1;
            continue;
        }

        match parse_chart(&path) {
            Ok(meta) => {
                // 区分新增与更新：先查存在性
                let exists = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM songs WHERE path = ?1)",
                        params![meta.path.to_string_lossy()],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap_or(0)
                    != 0;

                let res = if exists {
                    tx.execute(
                        "UPDATE songs SET
                            file_name=?2, title=?3, artist=?4, genre=?5,
                            play_level=?6, initial_bpm=?7, note_count=?8,
                            playable_count=?9, bgm_count=?10, bpm_change_count=?11,
                            stop_count=?12, measure_count=?13, warning_count=?14,
                            file_modified=?15
                         WHERE path = ?1",
                        params![
                            meta.path.to_string_lossy(),
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
                            mtime,
                        ],
                    )
                } else {
                    tx.execute(
                        "INSERT INTO songs (path, file_name, title, artist, genre, play_level,
                                            initial_bpm, note_count, playable_count, bgm_count,
                                            bpm_change_count, stop_count, measure_count,
                                            warning_count, file_modified)
                         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                        params![
                            meta.path.to_string_lossy(),
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
                            mtime,
                        ],
                    )
                };
                match res {
                    Ok(_) if exists => report.updated += 1,
                    Ok(_) => report.added += 1,
                    Err(e) => {
                        warn!("[song-database] 写入失败 {}: {e}", path.display());
                        report.failed += 1;
                    }
                }
            }
            Err(e) => {
                warn!("[song-database] 解析失败 {}: {e}", path.display());
                report.failed += 1;
            }
        }
    }

    // 清理已消失的铺面记录
    let stale_result = tx
        .prepare(
            "SELECT path FROM songs
             WHERE length(path) > length(?1) + 1
               AND substr(path, 1, length(?1) + 1) = ?1 || '/'",
        )
        .and_then(|mut stmt| {
            let rows = stmt.query_map(params![dir_str], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<String>, _>>()
        });
    let stale = match stale_result {
        Ok(stale) => stale,
        Err(e) => {
            warn!("[song-database] 查询失效记录失败: {e}");
            return;
        }
    };
    for path in stale {
        if !on_disk.contains(&path)
            && tx
                .execute("DELETE FROM songs WHERE path = ?1", params![path])
                .is_ok()
        {
            report.removed += 1;
        }
    }
}

/// Startup 阶段入口：打开 songs.db，首次启动时自动扫描默认目录，然后进入选曲。
fn bootstrap_database(
    mut commands: Commands,
    mut next: ResMut<NextState<AppState>>,
) {
    let db = match SongsDb::open() {
        Ok(db) => db,
        Err(e) => {
            error!("[song-database] 初始化失败: {e}");
            next.set(AppState::SongSelect);
            return;
        }
    };

    // 首次启动：没有任何已配置目录时，添加默认测试目录并执行首扫
    let needs_initial_scan = db.list_folders().map_or(true, |f| f.is_empty());
    if needs_initial_scan {
        if let Some(default) = default_test_folder() {
            info!(
                "[song-database] 首次启动，添加默认测试目录: {}",
                default.display()
            );
            let _ = db.add_folder(&default);
        }
        match db.scan() {
            Ok(report) => info!("[song-database] 首扫完成: {report}"),
            Err(e) => error!("[song-database] 首扫失败: {e}"),
        }
    }

    match db.count() {
        Ok(n) => info!("[song-database] songs.db 就绪，共 {n} 个铺面"),
        Err(e) => warn!("[song-database] 读取计数失败: {e}"),
    }
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

/// 解析单个铺面文件，提取元数据摘要。
fn parse_chart(path: &Path) -> Result<SongMeta, String> {
    let bytes = fs::read(path).map_err(|e| format!("读取失败: {e}"))?;
    let source = decode_bms(&bytes);
    let output = parse_bms(&source, default_config());
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
    })
}

/// 把铺面源文本解码为 UTF-8。
///
/// 优先按 UTF-8 读取；失败（典型为 Shift_JIS 日文铺面）时用 Shift_JIS 转码。
pub(crate) fn decode_bms(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => {
            let (cow, _, had_errors) = encoding_rs::SHIFT_JIS.decode(bytes);
            if had_errors {
                warn!("[song-database] 无法识别的编码（既非 UTF-8 也非 Shift_JIS）");
            }
            cow.into_owned()
        }
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
    }

    #[test]
    fn decode_shift_jis_text() {
        // 「ああああ」的 Shift_JIS 编码
        let sjis = [0x82u8, 0xA0, 0x82, 0xA0, 0x82, 0xA0, 0x82, 0xA0];
        assert_eq!(decode_bms(&sjis), "ああああ");
        // UTF-8 原样保留
        assert_eq!(decode_bms("hello".as_bytes()), "hello");
    }

    #[test]
    fn songs_db_roundtrip() {
        let _temp = TempDb::new("roundtrip");

        // 准备一个临时铺面目录 + 迷你铺面
        let chart_dir = _temp.0.join("charts");
        fs::create_dir_all(&chart_dir).unwrap();
        let chart = chart_dir.join("mini.bms");
        fs::write(&chart, MINI_BMS).unwrap();

        let db = SongsDb::open_at(_temp.db_path()).expect("打开数据库");
        db.add_folder(&chart_dir).expect("添加目录");

        // 扫描 → 记录应存在且字段正确
        let report = db.scan().expect("扫描");
        assert_eq!(report.added, 1, "应新增 1 个铺面: {report}");
        assert_eq!(report.unchanged, 0);

        let songs = db.list_songs().expect("查询铺面");
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].title.as_deref(), Some("Mini Chart"));
        assert_eq!(songs[0].playable_count, 1);

        // 再次扫描 → 未变化，应全部跳过
        let report2 = db.scan().expect("二次扫描");
        assert_eq!(report2.unchanged, 1, "应跳过未变化文件: {report2}");
        assert_eq!(report2.added, 0);

        // 修改文件 → 再次扫描应更新
        fs::write(&chart, MINI_BMS.replace("Lv.5", "").replace("#PLAYLEVEL 5", "#PLAYLEVEL 9"))
            .unwrap();
        // 部分文件系统 mtime 精度不足，等 10ms 确保 mtime 变化
        std::thread::sleep(std::time::Duration::from_millis(20));
        let report3 = db.scan().expect("三次扫描");
        assert_eq!(report3.updated, 1, "内容变化应更新: {report3}");
        let songs = db.list_songs().expect("查询");
        assert_eq!(songs[0].play_level, Some(9));

        // 删除文件 → 再次扫描应移除记录
        fs::remove_file(&chart).unwrap();
        let report4 = db.scan().expect("四次扫描");
        assert_eq!(report4.removed, 1, "应移除消失的铺面: {report4}");
        assert!(db.list_songs().expect("查询").is_empty());

        // 移除文件夹 → 配置消失
        db.remove_folder(&chart_dir).expect("移除目录");
        assert!(db.list_folders().expect("查询目录").is_empty());
    }

    /// 真实目录端到端：扫描 rainbow_ogg → songs.db → 查询（文件不存在时跳过）。
    #[test]
    fn scan_real_rainbow_dir() {
        let home = std::env::var("HOME").unwrap_or_default();
        let dir = PathBuf::from(home).join(".local/share/lr2oraja/songs/rainbow_ogg");
        if !dir.is_dir() {
            eprintln!("跳过：找不到真实铺面目录 {}", dir.display());
            return;
        }
        let _temp = TempDb::new("real_scan");
        let db = SongsDb::open_at(_temp.db_path()).expect("打开数据库");
        db.add_folder(&dir).expect("添加目录");

        let report = db.scan().expect("扫描");
        eprintln!("真实目录扫描: {report}");
        assert_eq!(report.scanned_dirs, 1);
        assert!(report.added >= 4, "rainbow 目录应有 ≥4 个铺面，实际 {report}");
        assert_eq!(report.failed, 0, "不应有解析失败: {report}");

        let songs = db.list_songs().expect("查询");
        assert!(
            songs.iter().any(|s| s.title.as_deref() == Some("rainbow")),
            "应能找到 title=rainbow 的铺面"
        );
        assert!(songs.iter().any(|s| s.file_name == "rainbowA.bms"));
    }

    /// 使用 lr2oraja 示例铺面验证真实解析（文件不存在时跳过）。
    #[test]
    fn parse_real_rainbow_chart() {
        let home = std::env::var("HOME").unwrap_or_default();
        let path = PathBuf::from(home).join(".local/share/lr2oraja/songs/rainbow_ogg/rainbowA.bms");
        if !path.exists() {
            eprintln!("跳过：找不到真实铺面 {}", path.display());
            return;
        }
        let meta = parse_chart(&path).expect("真实铺面应能解析");
        eprintln!("真实铺面解析结果: {meta}");
        assert_eq!(meta.title.as_deref(), Some("rainbow"));
        assert_eq!(meta.genre.as_deref(), Some("ELE POP"));
        assert!(meta.playable_count > 0, "应有可玩音符");
        assert!(meta.bgm_count > 0, "应有 BGM 音符（通道 01）");
        assert!(meta.measure_count > 1, "应有多于一个小节");
        assert!(meta.note_count >= meta.playable_count + meta.bgm_count);
    }
}
