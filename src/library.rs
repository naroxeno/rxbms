//! 铺面库运行时：按需后台扫描 worker。
//!
//! 设计：
//! - **不阻塞主线程**：扫描在独立线程执行，界面始终可交互（读缓存结果）。
//! - **只按需扫描**：不会自动、周期性地扫描整个曲库。唯一的触发点是用户
//!   显式操作——设置页「重新扫描」、添加/移除目录，或选曲页 F5。
//! - **请求合并**：请求队列容量有限，连续触发时自动丢弃重复请求。
//! - **进度上报**：解析阶段每 ~50ms 通过 [`LibraryProgress`] 上报一次，
//!   完成后广播 [`LibraryChanged`]，选曲/设置界面据此刷新。

use std::{
    path::PathBuf,
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use bevy::prelude::*;

use crate::database::{ScanReport, SongsDb};

/// 扫描完成（成功）后广播：界面据此刷新铺面列表。
#[derive(Message, Clone, Debug)]
pub struct LibraryChanged {
    /// 本次扫描统计。
    pub report: ScanReport,
}

/// 扫描进度（节流后上报，用于界面进度提示）。
#[derive(Message, Clone, Copy, Debug)]
pub struct LibraryProgress {
    /// 已解析文件数。
    pub done: usize,
    /// 本轮需要解析的文件总数。
    pub total: usize,
}

/// 扫描器对外状态（UI 只读）。
#[derive(Resource, Debug, Default)]
pub struct LibraryStatus {
    /// 是否正在扫描。
    pub scanning: bool,
    /// 已解析文件数。
    pub done: usize,
    /// 本轮需解析的文件总数。
    pub total: usize,
    /// 最近一次成功扫描的统计。
    pub last_report: Option<ScanReport>,
    /// 最近一次失败原因（成功扫描后清空）。
    pub last_error: Option<String>,
}

/// 扫描请求。
enum ScanRequest {
    /// 扫描全部已配置目录。
    All,
    /// 只重扫这些根目录。
    Roots(Vec<PathBuf>),
}

/// 扫描线程 → ECS 的事件。
enum WorkerEvent {
    /// 一轮扫描开始。
    Started,
    /// 解析进度。
    Progress { done: usize, total: usize },
    /// 扫描成功。
    Done(ScanReport),
    /// 扫描失败（数据库事务错误等）。
    Failed(String),
}

/// 后台扫描器（Resource）。
#[derive(Resource)]
pub struct SongScanner {
    requests: mpsc::SyncSender<ScanRequest>,
    events: Mutex<mpsc::Receiver<WorkerEvent>>,
}

impl SongScanner {
    /// 启动后台扫描线程。扫描只在被显式请求时进行。
    #[must_use]
    pub fn spawn(db: SongsDb) -> Self {
        // 请求队列：容量小，满了直接丢弃（合并重复扫描）
        let (request_tx, request_rx) = mpsc::sync_channel::<ScanRequest>(8);
        let (event_tx, event_rx) = mpsc::channel::<WorkerEvent>();

        let worker_db = db.clone();
        let worker_events = event_tx.clone();
        if let Err(e) = thread::Builder::new()
            .name(String::from("rxbms-scan"))
            .spawn(move || {
                for request in request_rx {
                    let _ = worker_events.send(WorkerEvent::Started);
                    let roots = match request {
                        ScanRequest::All => worker_db.list_folders().unwrap_or_default(),
                        ScanRequest::Roots(roots) => roots,
                    };
                    let mut last_emit = Instant::now() - Duration::from_secs(1);
                    let mut on_progress = |done: usize, total: usize| {
                        if done == total || last_emit.elapsed() >= Duration::from_millis(50) {
                            last_emit = Instant::now();
                            let _ = worker_events.send(WorkerEvent::Progress { done, total });
                        }
                    };
                    match worker_db.scan_roots_with_progress(&roots, &mut on_progress) {
                        Ok(report) => {
                            let _ = worker_events.send(WorkerEvent::Done(report));
                        }
                        Err(error) => {
                            let _ = worker_events.send(WorkerEvent::Failed(error));
                        }
                    }
                }
            })
        {
            error!("[library] 启动扫描线程失败: {e}");
        }

        Self {
            requests: request_tx,
            events: Mutex::new(event_rx),
        }
    }

    /// 请求全量增量扫描（设置页「重新扫描」/ 选曲页 F5）。
    pub fn request_scan_all(&self) {
        if self.requests.try_send(ScanRequest::All).is_err() {
            debug!("[library] 扫描请求队列已满，忽略本次全量扫描请求");
        }
    }

    /// 请求重扫指定根目录（新增目录后）。
    pub fn request_scan_roots(&self, roots: Vec<PathBuf>) {
        if roots.is_empty() {
            return;
        }
        if self.requests.try_send(ScanRequest::Roots(roots)).is_err() {
            debug!("[library] 扫描请求队列已满，忽略本次增量扫描请求");
        }
    }

}

/// 每帧把扫描线程的事件转成 Bevy 消息 / 状态更新。
fn drain_scanner_events(
    scanner: Option<Res<SongScanner>>,
    db: Option<Res<SongsDb>>,
    mut status: ResMut<LibraryStatus>,
    mut changed: MessageWriter<LibraryChanged>,
    mut progress: MessageWriter<LibraryProgress>,
) {
    let Some(scanner) = scanner else {
        return;
    };
    let Ok(receiver) = scanner.events.lock() else {
        return;
    };
    while let Ok(event) = receiver.try_recv() {
        match event {
            WorkerEvent::Started => {
                status.scanning = true;
                status.done = 0;
                status.total = 0;
                status.last_error = None;
            }
            WorkerEvent::Progress { done, total } => {
                status.scanning = true;
                status.done = done;
                status.total = total;
                progress.write(LibraryProgress { done, total });
            }
            WorkerEvent::Done(report) => {
                status.scanning = false;
                status.last_report = Some(report);
                status.last_error = None;
                info!("[library] {report}");
                if report.failed > 0
                    && let Some(db) = db.as_deref()
                    && let Ok(errors) = db.list_scan_errors()
                {
                    for (path, error) in errors.iter().take(5) {
                        warn!("[library] 解析失败 {}: {error}", path.display());
                    }
                }
                changed.write(LibraryChanged { report });
            }
            WorkerEvent::Failed(error) => {
                status.scanning = false;
                error!("[library] 扫描失败: {error}");
                status.last_error = Some(error);
            }
        }
    }
}

/// 铺面库插件。
pub struct SongLibraryPlugin;

impl Plugin for SongLibraryPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<LibraryChanged>()
            .add_message::<LibraryProgress>()
            .init_resource::<LibraryStatus>()
            .add_systems(Update, drain_scanner_events);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 后台扫描线程：请求 → Started/Progress/Done，且结果写入数据库。
    #[test]
    fn background_scan_reports_and_persists() {
        let root = std::env::temp_dir().join(format!("rxbms_scanner_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let chart = root.join("a.bms");
        std::fs::write(
            &chart,
            "#TITLE Scan Test\n#BPM 120\n#WAV01 k.wav\n#00111:01\n",
        )
        .unwrap();
        let db = SongsDb::open_at(root.join("songs.db")).unwrap();
        db.add_folder(&root).expect("登记目录");
        let scanner = SongScanner::spawn(db.clone());
        scanner.request_scan_all();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut done = None;
        while std::time::Instant::now() < deadline {
            if let Ok(events) = scanner.events.lock() {
                while let Ok(event) = events.try_recv() {
                    if let WorkerEvent::Done(report) = event {
                        done = Some(report);
                    }
                }
            }
            if done.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let report = done.expect("5 秒内应收到扫描完成事件");
        assert_eq!(report.added, 1, "应索引一个铺面: {report}");
        assert_eq!(db.count().unwrap(), 1, "扫描结果应写入数据库");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 只按需扫描：spawn 之后空闲等待不会自己扫描任何东西。
    #[test]
    fn idle_scanner_does_not_scan() {
        let root = std::env::temp_dir().join(format!("rxbms_idle_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("a.bms"),
            "#TITLE Idle\n#BPM 120\n#WAV01 k.wav\n#00111:01\n",
        )
        .unwrap();
        let db = SongsDb::open_at(root.join("songs.db")).unwrap();
        db.add_folder(&root).expect("登记目录");

        let scanner = SongScanner::spawn(db.clone());
        std::thread::sleep(std::time::Duration::from_millis(400));
        assert_eq!(
            db.count().unwrap(),
            0,
            "未经显式请求不应发生任何扫描"
        );
        if let Ok(events) = scanner.events.lock() {
            assert!(
                matches!(
                    events.try_recv(),
                    Err(std::sync::mpsc::TryRecvError::Empty)
                ),
                "空闲扫描器不应产生任何事件"
            );
        }

        // 显式请求后才扫描
        scanner.request_scan_all();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut report = None;
        while std::time::Instant::now() < deadline && report.is_none() {
            if let Ok(events) = scanner.events.lock()
                && let Ok(WorkerEvent::Done(done)) = events.try_recv()
            {
                report = Some(done);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(report.expect("显式请求后应扫描").added, 1);
        assert_eq!(db.count().unwrap(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
